import asyncio
from typing import assert_type

from oxiserial import Serial, serial_for_url
from oxiserial.aio import Future, create_serial_connection, open_serial_connection
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


async def serial_asyncio_api(loop: asyncio.AbstractEventLoop) -> None:
    """Type the transport as an asyncio.Transport and the streams as asyncio's own."""
    transport, protocol = await create_serial_connection(
        loop, asyncio.Protocol, "loop://"
    )
    as_transport: asyncio.Transport = transport
    protocol.connection_made(as_transport)
    reader, writer = await open_serial_connection(url="loop://")
    assert_type(reader, asyncio.StreamReader)
    assert_type(writer, asyncio.StreamWriter)
