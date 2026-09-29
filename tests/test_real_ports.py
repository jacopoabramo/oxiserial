import asyncio
import threading
import time

import pytest

from conftest import Runner
from oxiserial import Serial
from oxiserial.aio import Serial as AioSerial


def test_round_trip(real_pair: tuple[str, str]) -> None:
    """Exchange text and all 256 byte values between two real ports."""
    with (
        Serial(real_pair[0], 115200, timeout=1) as a,
        Serial(real_pair[1], 115200, timeout=1) as b,
    ):
        a.write(b"ping\n")
        assert b.readline() == b"ping\n"
        b.write(bytes(range(256)))
        assert a.read(256) == bytes(range(256))


def test_timeout_on_a_silent_port(real_pair: tuple[str, str]) -> None:
    """Return no bytes when nothing arrives before the timeout."""
    with Serial(real_pair[0], timeout=0.1) as a:
        assert a.read(10) == b""


@pytest.mark.parametrize("parity", ["N", "E", "O"])
def test_settings_apply_while_open(real_pair: tuple[str, str], parity: str) -> None:
    """Apply parity and baud rate changes to an open real port."""
    with Serial(real_pair[0]) as a:
        a.parity = parity
        a.baudrate = 57600
        assert a.parity == parity
        assert a.baudrate == 57600


def test_async_round_trip(real_pair: tuple[str, str], run: Runner) -> None:
    """Exchange a line between two async real ports."""

    async def main() -> bytes:
        async with (
            AioSerial(real_pair[0], timeout=1) as a,
            AioSerial(real_pair[1], timeout=1) as b,
        ):
            await a.write(b"async\n")
            return await b.readline()

    assert run(main()) == b"async\n"


def test_reads_right_after_open_and_after_a_reset(real_pair: tuple[str, str]) -> None:
    """Read fresh data right after opening and after an input reset."""
    with Serial(real_pair[0], timeout=1) as a, Serial(real_pair[1], timeout=1) as b:
        a.write(b"first")
        assert b.read(5) == b"first"
        a.write(b"stale")
        time.sleep(0.05)
        b.reset_input_buffer()
        assert b.in_waiting == 0
        a.write(b"fresh")
        assert b.read(5) == b"fresh"


def test_in_waiting_counts_bytes_from_the_peer(real_pair: tuple[str, str]) -> None:
    """Count the bytes sent by the peer in in_waiting."""
    with Serial(real_pair[0], timeout=1) as a, Serial(real_pair[1], timeout=1) as b:
        a.write(b"abc")
        deadline = time.monotonic() + 1
        while b.in_waiting < 3 and time.monotonic() < deadline:
            time.sleep(0.01)
        assert b.in_waiting == 3
        assert b.read(3) == b"abc"


def test_a_pending_read_wakes_promptly(real_pair: tuple[str, str]) -> None:
    """Wake a pending read within milliseconds of the peer's write."""
    with Serial(real_pair[0], timeout=1) as a, Serial(real_pair[1], timeout=2) as b:
        written: list[float] = []

        def write_later() -> None:
            time.sleep(0.2)
            written.append(time.monotonic())
            a.write(b"x")

        writer = threading.Thread(target=write_later)
        writer.start()
        assert b.read(1) == b"x"
        woke = time.monotonic()
        writer.join()
        assert woke - written[0] < 0.05


def test_a_cancelled_read_leaves_later_data(
    real_pair: tuple[str, str], run: Runner
) -> None:
    """Keep bytes that arrive after a read was cancelled."""

    async def main() -> bytes:
        async with AioSerial(real_pair[0]) as a, AioSerial(real_pair[1]) as b:
            with pytest.raises(TimeoutError):
                await asyncio.wait_for(b.read(3), 0.1)
            await a.write(b"abc")
            return await asyncio.wait_for(b.read(3), 2)

    assert run(main()) == b"abc"


def test_a_non_blocking_write_held_by_flow_control_is_sent_later(
    real_pair: tuple[str, str],
) -> None:
    """Send a non-blocking write held by flow control once the peer allows it."""
    with Serial(real_pair[1], timeout=2) as b:
        b.rts = False
        with Serial(real_pair[0], rtscts=True, write_timeout=0) as a:
            assert a.write(b"x" * 100) == 100
            b.rts = True
            assert b.read(100) == b"x" * 100


def test_reconfiguring_keeps_lowered_lines_low(real_pair: tuple[str, str]) -> None:
    """Keep lowered RTS and DTR low through repeated baud rate and timeout changes."""
    a = Serial()
    a.port = real_pair[0]
    a.dtr = False
    a.rts = False
    seen_high: list[str] = []
    done = threading.Event()
    with Serial(real_pair[1]) as b:
        a.open()

        def poll() -> None:
            while not done.is_set():
                if b.dsr:
                    seen_high.append("dsr")
                if b.cts:
                    seen_high.append("cts")

        watcher = threading.Thread(target=poll)
        watcher.start()
        try:
            for i in range(200):
                a.baudrate = 19200 if i % 2 else 9600
            for i in range(200):
                a.timeout = 0.5 if i % 2 else 1.0
        finally:
            done.set()
            watcher.join()
            a.close()
    assert seen_high == []
