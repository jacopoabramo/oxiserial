from typing import assert_type

from oxiserial import Serial, serial_for_url
from oxiserial.aio import Future
from oxiserial.aio import Serial as AioSerial
from oxiserial.aio import serial_for_url as aio_serial_for_url


def sync_api(port: Serial) -> None:
    """Type the blocking methods to return bytes, int and lists of bytes."""
    assert_type(port.read(), bytes)
    assert_type(port.write(b"x"), int)
    assert_type(port.write("x"), int)
    assert_type(port.readlines(), list[bytes])
    with port as opened:
        assert_type(opened, Serial)


async def aio_api(port: AioSerial) -> None:
    """Type the async methods to return futures that resolve to plain values."""
    assert_type(port.read(), Future[bytes])
    assert_type(port.read().wait(), bytes)
    assert_type(await port.read(), bytes)
    assert_type(await port.write(b"x"), int)
    assert_type(await port.write("x"), int)
    async with port as opened:
        assert_type(opened, AioSerial)


def url_api() -> None:
    """Type `serial_for_url` to return the Serial class of its module."""
    assert_type(serial_for_url("loop://"), Serial)
    assert_type(aio_serial_for_url("loop://"), AioSerial)
