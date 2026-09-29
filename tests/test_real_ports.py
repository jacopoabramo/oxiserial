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
