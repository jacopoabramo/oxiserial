from typing import assert_type

from oxiserial import Serial
from oxiserial.aio import Future
from oxiserial.aio import Serial as AioSerial


def sync_api(port: Serial) -> None:
    assert_type(port.read(), bytes)
    assert_type(port.write(b"x"), int)
    assert_type(port.write("x"), int)
    assert_type(port.readlines(), list[bytes])
    with port as opened:
        assert_type(opened, Serial)


async def aio_api(port: AioSerial) -> None:
    assert_type(port.read(), Future[bytes])
    assert_type(port.read().wait(), bytes)
    assert_type(await port.read(), bytes)
    assert_type(await port.write(b"x"), int)
    assert_type(await port.write("x"), int)
    async with port as opened:
        assert_type(opened, AioSerial)
