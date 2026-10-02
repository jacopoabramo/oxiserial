import asyncio
import os
import sys
import threading
import traceback
from collections.abc import Callable
from typing import Any

import pytest

from conftest import Runner
from oxiserial import Serial, SerialException, _testing
from oxiserial import aio as serial_asyncio


class Recorder(asyncio.Protocol):
    """A protocol that records every callback its transport makes, in order."""

    def __init__(self) -> None:
        self.events: list[tuple[str, Any]] = []
        self.data = bytearray()
        self.lost: asyncio.Future[Exception | None] | None = None

    def connection_made(self, transport: asyncio.BaseTransport) -> None:
        self.events.append(("made", transport))
        self.lost = asyncio.get_running_loop().create_future()

    def data_received(self, data: bytes) -> None:
        self.events.append(("data", data))
        self.data += data

    def pause_writing(self) -> None:
        self.events.append(("pause", None))

    def resume_writing(self) -> None:
        self.events.append(("resume", None))

    def connection_lost(self, exc: Exception | None) -> None:
        self.events.append(("lost", exc))
        if self.lost is not None and not self.lost.done():
            self.lost.set_result(exc)

    async def closed(self) -> Exception | None:
        """Wait for `connection_lost` and return its argument."""
        await until(lambda: self.lost is not None)
        assert self.lost is not None
        return await asyncio.wait_for(self.lost, 5)


async def until(ready: Callable[[], object]) -> None:
    """Poll `ready` on the loop for up to 5 seconds."""
    for _ in range(500):
        if ready():
            return
        await asyncio.sleep(0.01)
    raise AssertionError("condition not reached within 5 s")


async def connect(name: str) -> tuple[Any, Recorder]:
    """Open `name` through create_serial_connection with a Recorder."""
    recorder = Recorder()
    transport, protocol = await serial_asyncio.create_serial_connection(
        asyncio.get_running_loop(), lambda: recorder, name
    )
    assert protocol is recorder
    await until(lambda: recorder.events)
    return transport, recorder


def test_open_serial_connection_round_trips_a_line(run: Runner) -> None:
    """Write a line through a StreamWriter and read it back from the StreamReader."""

    async def main() -> bytes:
        reader, writer = await serial_asyncio.open_serial_connection(
            url="loop://", baudrate=115200
        )
        writer.write(b"hello\n")
        await writer.drain()
        line = await asyncio.wait_for(reader.readline(), 5)
        writer.close()
        await writer.wait_closed()
        return line

    assert run(main()) == b"hello\n"


def test_protocol_sees_connection_data_and_close(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Call connection_made, deliver data in order, then connection_lost on close."""
    peer = Serial(mock_pair[1], timeout=1)
    payload = bytes(range(256)) * 40

    async def main() -> tuple[Any, Recorder, Exception | None]:
        transport, recorder = await connect(mock_pair[0])
        for i in range(0, len(payload), 1000):
            peer.write(payload[i : i + 1000])
        await until(lambda: len(recorder.data) >= len(payload))
        transport.close()
        return transport, recorder, await recorder.closed()

    try:
        transport, recorder, exc = run(main())
    finally:
        peer.close()
    assert recorder.events[0] == ("made", transport)
    assert bytes(recorder.data) == payload
    assert exc is None
    assert recorder.events[-1] == ("lost", None)
    assert not transport.serial.is_open


def test_create_task_accepts_the_coroutine(run: Runner) -> None:
    """Schedule create_serial_connection with create_task and use the transport."""

    async def main() -> None:
        recorder = Recorder()
        task = asyncio.create_task(
            serial_asyncio.create_serial_connection(
                asyncio.get_running_loop(), lambda: recorder, "loop://"
            )
        )
        transport, _ = await task
        assert transport.get_extra_info("serial") is transport.serial
        assert transport.get_extra_info("other", 5) == 5
        assert transport.can_write_eof() is False
        with pytest.raises(NotImplementedError):
            transport.write_eof()
        assert transport.serial.timeout == 0
        assert transport.serial.write_timeout == 0
        transport.close()
        # Waiting for connection_lost keeps a done-callback from outliving the loop.
        await recorder.closed()

    run(main())


def test_write_buffer_limits_round_like_pyserial_asyncio(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Derive low as high // 4, rounding down, and name both values in the error."""

    async def main() -> None:
        transport, recorder = await connect(mock_pair[0])
        with pytest.raises(ValueError, match=r"^high \(-1\) must be >= low \(-1\)"):
            transport.set_write_buffer_limits(high=-1)
        transport.close()
        await recorder.closed()

    run(main())


@pytest.mark.filterwarnings("ignore:This process:DeprecationWarning")
def test_write_in_a_forked_child_closes_the_transport(
    mock_pair: tuple[str, str],
) -> None:
    """Close the transport and drop its buffer when a forked child writes to it."""
    if sys.platform == "win32":
        pytest.skip("needs fork()")
    loop = asyncio.new_event_loop()
    transport, recorder = loop.run_until_complete(connect(mock_pair[0]))
    try:
        pid = os.fork()
        if pid == 0:
            # The child cannot run the loop: macOS does not pass its kqueue to a child.
            code = 1
            try:
                transport.write(b"x")
                if transport.is_closing() and transport.get_write_buffer_size() == 0:
                    # As connection_lost would; the pending read must not wake.
                    transport.serial.close()
                    code = 0
                else:
                    print("the transport is still open", file=sys.stderr)
            except BaseException:
                traceback.print_exc()
            finally:
                os._exit(code)
        _, status = os.waitpid(pid, 0)
        assert os.waitstatus_to_exitcode(status) == 0
    finally:
        transport.close()
        loop.run_until_complete(recorder.closed())
        loop.close()


def test_write_flow_control_pauses_and_resumes(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Pause the protocol above the high limit and resume it once the bytes are sent."""
    peer = Serial(mock_pair[1], timeout=1)
    _testing.mock_block_writes(mock_pair[0], True)

    async def main() -> Recorder:
        transport, recorder = await connect(mock_pair[0])
        transport.set_write_buffer_limits(high=16, low=4)
        transport.write(b"x" * 64)
        assert ("pause", None) in recorder.events
        assert transport.get_write_buffer_size() == 64
        _testing.mock_block_writes(mock_pair[0], False)
        await until(lambda: ("resume", None) in recorder.events)
        transport.close()
        await recorder.closed()
        return recorder

    try:
        run(main())
        assert peer.read(64) == b"x" * 64
    finally:
        peer.close()


def test_pause_reading_holds_data_until_resumed(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Deliver nothing while reading is paused and everything in order after."""
    peer = Serial(mock_pair[1], timeout=1)

    async def main() -> bytes:
        transport, recorder = await connect(mock_pair[0])
        transport.pause_reading()
        peer.write(b"abc")
        await asyncio.sleep(0.2)
        assert recorder.data == b""
        transport.resume_reading()
        peer.write(b"def")
        await until(lambda: len(recorder.data) >= 6)
        transport.close()
        await recorder.closed()
        return bytes(recorder.data)

    try:
        assert run(main()) == b"abcdef"
    finally:
        peer.close()


def test_abort_drops_queued_bytes(mock_pair: tuple[str, str], run: Runner) -> None:
    """Send none of the queued or in-flight bytes after abort."""
    peer = Serial(mock_pair[1], timeout=0.3)
    _testing.mock_block_writes(mock_pair[0], True)

    async def main() -> Exception | None:
        transport, recorder = await connect(mock_pair[0])
        transport.write(b"lost")
        transport.write(b"more")
        transport.abort()
        return await recorder.closed()

    try:
        assert run(main()) is None
        _testing.mock_block_writes(mock_pair[0], False)
        assert peer.read(8) == b""
    finally:
        peer.close()


def test_flush_discards_the_queue(mock_pair: tuple[str, str], run: Runner) -> None:
    """Discard queued bytes on flush while the write in flight still goes out."""
    peer = Serial(mock_pair[1], timeout=0.3)
    _testing.mock_block_writes(mock_pair[0], True)

    async def main() -> None:
        transport, recorder = await connect(mock_pair[0])
        transport.write(b"a" * 8)
        transport.write(b"b" * 8)
        transport.flush()
        assert transport.get_write_buffer_size() == 8
        _testing.mock_block_writes(mock_pair[0], False)
        await until(lambda: transport.get_write_buffer_size() == 0)
        transport.close()
        await recorder.closed()

    try:
        run(main())
        assert peer.read(16) == b"a" * 8
    finally:
        peer.close()


def test_unplugged_device_ends_the_connection(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Call connection_lost with a SerialException when the device goes away."""

    async def main() -> tuple[Any, Exception | None]:
        transport, recorder = await connect(mock_pair[0])
        _testing.mock_unplug(mock_pair[0])
        return transport, await recorder.closed()

    transport, exc = run(main())
    assert isinstance(exc, SerialException)
    assert not transport.serial.is_open


def test_stream_reader_limit_keeps_every_byte_in_order(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Deliver every byte in order while a small reader limit pauses reading."""
    peer = Serial(mock_pair[1], timeout=1)
    payload = bytes(i % 251 for i in range(200_000))

    def send() -> None:
        for i in range(0, len(payload), 97):
            peer.write(payload[i : i + 97])

    async def main() -> bytes:
        reader, writer = await serial_asyncio.open_serial_connection(
            url=mock_pair[0], limit=64
        )
        sender = threading.Thread(target=send)
        sender.start()
        received = bytearray()
        while len(received) < len(payload):
            received += await asyncio.wait_for(reader.read(1000), 5)
        sender.join()
        writer.close()
        await writer.wait_closed()
        return bytes(received)

    try:
        assert run(main()) == payload
    finally:
        peer.close()


def test_transport_is_an_asyncio_transport(run: Runner) -> None:
    """Hand the protocol and the caller an asyncio.Transport."""

    async def main() -> None:
        recorder = Recorder()
        transport, _ = await serial_asyncio.create_serial_connection(
            asyncio.get_running_loop(), lambda: recorder, "loop://"
        )
        await until(lambda: recorder.events)
        assert isinstance(transport, asyncio.Transport)
        assert recorder.events[0] == ("made", transport)
        assert issubclass(serial_asyncio.SerialTransport, asyncio.Transport)
        transport.close()
        await recorder.closed()

    run(main())
