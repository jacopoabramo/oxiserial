import queue
import threading
import time
from typing import Any

import pytest

from conftest import wait_until
from oxiserial import serial_for_url
from oxiserial.threaded import (
    FramedPacket,
    LineReader,
    Packetizer,
    Protocol,
    ReaderThread,
)


class Lines(LineReader):
    """Collect the lines received and the argument of connection_lost."""

    def __init__(self) -> None:
        super().__init__()
        self.lines: queue.Queue[str] = queue.Queue()
        self.lost: queue.Queue[Exception | None] = queue.Queue()

    def handle_line(self, line: str) -> None:
        self.lines.put(line)

    def connection_lost(self, exc: Exception | None) -> None:
        self.lost.put(exc)


class Packets(Packetizer):
    """Collect the packets received."""

    def __init__(self) -> None:
        super().__init__()
        self.packets: list[bytearray] = []

    def handle_packet(self, packet: bytearray) -> None:
        self.packets.append(packet)


class Frames(FramedPacket):
    """Collect framed packets and the bytes outside them."""

    def __init__(self) -> None:
        super().__init__()
        self.packets: list[bytes] = []
        self.outside = bytearray()

    def handle_packet(self, packet: bytes) -> None:
        self.packets.append(packet)

    def handle_out_of_packet_data(self, data: bytes) -> None:
        self.outside += data


class Failing(Protocol):
    """Fail on the first data and record the argument of connection_lost."""

    def __init__(self) -> None:
        self.lost: queue.Queue[Exception | None] = queue.Queue()

    def data_received(self, data: bytes) -> None:
        raise ValueError("bad data")

    def connection_lost(self, exc: Exception | None) -> None:
        self.lost.put(exc)


class Refusing(LineReader):
    """Fail in connection_made; the inherited connection_lost raises again."""

    def connection_made(self, transport: ReaderThread[Any]) -> None:
        raise ValueError("refused")


class Blocking(Protocol):
    """Hold data_received until released."""

    def __init__(self) -> None:
        self.entered = threading.Event()
        self.release = threading.Event()

    def data_received(self, data: bytes) -> None:
        self.entered.set()
        self.release.wait(5)


class Replying(Lines):
    """Answer each line once released."""

    def __init__(self) -> None:
        super().__init__()
        self.entered = threading.Event()
        self.release = threading.Event()

    def handle_line(self, line: str) -> None:
        self.entered.set()
        self.release.wait(5)
        self.write_line("ack")


def test_line_reader_round_trip_on_loop() -> None:
    """Receive a written line through a LineReader and report a clean close."""
    port = serial_for_url("loop://")
    with ReaderThread(port, Lines) as protocol:
        protocol.write_line("hello")
        assert protocol.lines.get(timeout=5) == "hello"
    assert protocol.lost.get(timeout=5) is None
    assert not port.is_open


def test_packetizer_reassembles_split_packets() -> None:
    """Join bytes split across reads into packets that end at the terminator."""
    packets = Packets()
    for chunk in (b"ab\0c", b"d", b"\0e"):
        packets.data_received(chunk)
    assert packets.packets == [b"ab", b"cd"]
    assert packets.buffer == b"e"


def test_framed_packet_separates_frames_from_other_bytes() -> None:
    """Pass bytes between START and STOP as packets and the rest separately."""
    frames = Frames()
    frames.data_received(b"x(ab")
    frames.data_received(b")y()")
    assert frames.packets == [b"ab", b""]
    assert frames.outside == b"xy"


def test_an_error_in_data_received_ends_the_thread() -> None:
    """Pass an exception from data_received to connection_lost and end the thread."""
    port = serial_for_url("loop://")
    thread = ReaderThread(port, Failing)
    thread.start()
    _, protocol = thread.connect()
    port.write(b"x")
    assert isinstance(protocol.lost.get(timeout=5), ValueError)
    thread.join(5)
    assert not thread.is_alive()
    port.close()


@pytest.mark.filterwarnings("ignore::pytest.PytestUnhandledThreadExceptionWarning")
def test_a_failed_connection_made_raises_instead_of_hanging() -> None:
    """Raise RuntimeError on entry when connection_made fails and is raised again."""
    port = serial_for_url("loop://")
    thread = ReaderThread(port, Refusing)
    with pytest.raises(RuntimeError), thread:
        pass
    # The re-raised error reaches the thread hook only once the thread ends.
    thread.join(5)
    port.close()


def test_stop_during_data_received_leaves_the_port_usable() -> None:
    """Keep the port open after stop, with the next read waiting for data."""
    port = serial_for_url("loop://", timeout=2)
    thread = ReaderThread(port, Blocking)
    thread.start()
    _, protocol = thread.connect()
    port.write(b"x")
    assert protocol.entered.wait(5)
    stopper = threading.Thread(target=thread.stop)
    stopper.start()
    wait_until(lambda: not thread.alive, "stop() clearing alive")
    protocol.release.set()
    stopper.join(5)
    assert not thread.is_alive()
    assert port.is_open
    writer = threading.Timer(0.2, port.write, (b"y",))
    writer.start()
    assert port.read(1) == b"y"
    writer.join()
    port.close()


def test_close_lets_a_reply_finish_without_the_join_timeout() -> None:
    """Let a handler write its reply when close arrives, and close in under 1 s."""
    port = serial_for_url("loop://")
    thread = ReaderThread(port, Replying)
    thread.start()
    _, protocol = thread.connect()
    port.write(b"ping\r\n")
    assert protocol.entered.wait(5)
    closer = threading.Thread(target=thread.close)
    start = time.monotonic()
    closer.start()
    wait_until(lambda: not thread.alive, "close() stopping the reader")
    protocol.release.set()
    closer.join(5)
    assert time.monotonic() - start < 1
    assert protocol.lost.get(timeout=5) is None
