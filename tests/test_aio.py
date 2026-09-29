import asyncio
from typing import Any

import pytest

from conftest import Runner
from oxiserial.aio import Serial


def test_async_read_and_write(mock_pair: tuple[str, str], run: Runner) -> None:
    async def main() -> bytes:
        async with (
            Serial(mock_pair[0], timeout=1) as a,
            Serial(mock_pair[1], timeout=1) as b,
        ):
            assert await a.write(b"hi") == 2
            return await b.read(2)

    assert run(main()) == b"hi"


def test_futures_from_sync_code(mock_pair: tuple[str, str]) -> None:
    a = Serial(mock_pair[0], timeout=1)
    b = Serial(mock_pair[1], timeout=1)
    a.write(b"x\n").wait()
    assert b.readline().wait() == b"x\n"
    a.close()
    b.close()


def test_cancelled_read_leaves_later_data(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    async def main() -> bytes:
        async with Serial(mock_pair[0]) as a, Serial(mock_pair[1]) as b:
            with pytest.raises(TimeoutError):
                await asyncio.wait_for(b.read(3), 0.05)
            await a.write(b"abc")
            return await asyncio.wait_for(b.read(3), 2)

    assert run(main()) == b"abc"


def test_read_and_write_overlap(mock_pair: tuple[str, str], run: Runner) -> None:
    async def main() -> list[object]:
        async with (
            Serial(mock_pair[0], timeout=1) as a,
            Serial(mock_pair[1], timeout=1) as b,
        ):
            return list(await asyncio.gather(b.read(3), b.write(b"z"), a.write(b"abc")))

    assert run(main()) == [b"abc", 1, 3]


def test_write_str_is_utf8(mock_pair: tuple[str, str], run: Runner) -> None:
    async def main() -> tuple[int, bytes]:
        async with (
            Serial(mock_pair[0], timeout=1) as a,
            Serial(mock_pair[1], timeout=1) as b,
        ):
            return await a.write("h\u00e9"), await b.read(3)

    assert run(main()) == (3, b"h\xc3\xa9")


def test_subclass_can_add_parameters_under_async_with(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    class MySerial(Serial):
        def __init__(self, port: str, extra: str, **kwargs: Any) -> None:
            super().__init__(port, **kwargs)
            self.extra = extra

    async def main() -> tuple[str, bytes]:
        async with (
            MySerial(mock_pair[0], "x", timeout=1) as a,
            Serial(mock_pair[1], timeout=1) as b,
        ):
            assert a.is_open
            await a.write(b"hi")
            return a.extra, await b.read(2)

    assert run(main()) == ("x", b"hi")


def test_exit_closes_the_port(mock_pair: tuple[str, str], run: Runner) -> None:
    async def main() -> bool:
        async with Serial(mock_pair[0]) as a:
            pass
        return a.is_open

    assert run(main()) is False
