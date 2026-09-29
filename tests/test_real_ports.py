import asyncio
import threading
import time

import pytest

from conftest import Runner
from oxiserial import Serial
from oxiserial.aio import Serial as AioSerial


def test_round_trip(real_pair: tuple[str, str]) -> None:
    with (
        Serial(real_pair[0], 115200, timeout=1) as a,
        Serial(real_pair[1], 115200, timeout=1) as b,
    ):
        a.write(b"ping\n")
        assert b.readline() == b"ping\n"
        b.write(bytes(range(256)))
        assert a.read(256) == bytes(range(256))


def test_timeout_on_a_silent_port(real_pair: tuple[str, str]) -> None:
    with Serial(real_pair[0], timeout=0.1) as a:
        assert a.read(10) == b""


@pytest.mark.parametrize("parity", ["N", "E", "O"])
def test_settings_apply_while_open(real_pair: tuple[str, str], parity: str) -> None:
    with Serial(real_pair[0]) as a:
        a.parity = parity
        a.baudrate = 57600
        assert a.parity == parity
        assert a.baudrate == 57600


def test_async_round_trip(real_pair: tuple[str, str], run: Runner) -> None:
    async def main() -> bytes:
        async with (
            AioSerial(real_pair[0], timeout=1) as a,
            AioSerial(real_pair[1], timeout=1) as b,
        ):
            await a.write(b"async\n")
            return await b.readline()

    assert run(main()) == b"async\n"


def test_reads_right_after_open_and_after_a_reset(real_pair: tuple[str, str]) -> None:
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
    with Serial(real_pair[0], timeout=1) as a, Serial(real_pair[1], timeout=1) as b:
        a.write(b"abc")
        deadline = time.monotonic() + 1
        while b.in_waiting < 3 and time.monotonic() < deadline:
            time.sleep(0.01)
        assert b.in_waiting == 3
        assert b.read(3) == b"abc"


def test_a_pending_read_wakes_promptly(real_pair: tuple[str, str]) -> None:
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
    async def main() -> bytes:
        async with AioSerial(real_pair[0]) as a, AioSerial(real_pair[1]) as b:
            with pytest.raises(TimeoutError):
                await asyncio.wait_for(b.read(3), 0.1)
            await a.write(b"abc")
            return await asyncio.wait_for(b.read(3), 2)

    assert run(main()) == b"abc"
