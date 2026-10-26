[![PyPI](https://img.shields.io/pypi/v/oxiserial.svg?color=green)](https://pypi.org/project/oxiserial)
[![PyPI - Python Version](https://img.shields.io/pypi/pyversions/oxiserial)](https://pypi.org/project/oxiserial)
[![PyPI - Status](https://img.shields.io/pypi/status/oxiserial)](https://pypi.org/project/oxiserial)
[![License](https://img.shields.io/badge/License-BSD%203--Clause-blue.svg)](https://opensource.org/licenses/BSD-3-Clause)
[![CI](https://github.com/jacopoabramo/oxiserial/actions/workflows/ci.yaml/badge.svg?branch=main)](https://github.com/jacopoabramo/oxiserial/actions/workflows/ci.yaml)
[![uv](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/uv/main/assets/badge/v0.json)](https://github.com/astral-sh/uv)
[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json)](https://github.com/astral-sh/ruff)
[![Checked with mypy](https://www.mypy-lang.org/static/mypy_badge.svg)](https://mypy-lang.org/)
[![Conventional Commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white)](https://www.conventionalcommits.org)

# oxiserial

Serial port access for Python, written in Rust. It is almost a drop-in
replacement for [pyserial](https://pypi.org/project/pyserial/) 3.5, and adds
an asyncio interface whose I/O calls return futures. The few places where
it departs from pyserial are listed under
[Differences from pyserial](#differences-from-pyserial).

Runs on Windows, Linux and macOS with CPython 3.11 or newer, including the
free-threaded build.

## Install

```sh
pip install oxiserial
```

Wheels are published for Windows, Linux and macOS. On other platforms pip
builds from source, which needs a Rust toolchain.

## Replace pyserial

Change the import. Code that uses none of the
[differences](#differences-from-pyserial) runs as it is.

```python
import oxiserial as serial

with serial.Serial("COM3", 115200, timeout=1) as port:
    port.write(b"*IDN?\n")
    print(port.readline())
```

Standard baud rates are available as an enum, and any other rate the
driver supports is accepted as an int:

```python
from oxiserial import Baudrate, Serial

port = Serial("/dev/ttyUSB0", Baudrate.B115200)
port.baudrate = 250000
```

## Read and write from asyncio

`oxiserial.aio.Serial` takes the same arguments. Its I/O methods return a
future that you can await:

```python
import asyncio

from oxiserial.aio import Serial


async def main() -> None:
    async with Serial("/dev/ttyUSB0", 115200, timeout=1) as port:
        await port.write(b"*IDN?\n")
        print(await port.readline())


asyncio.run(main())
```

The same future can be waited on from plain threads, without an event loop:

```python
from oxiserial.aio import Serial

port = Serial("COM3", 115200, timeout=1)
reply = port.readline()
port.write(b"*IDN?\n").wait()
print(reply.wait(timeout=2))
port.close()
```

## Replace pyserial-asyncio

`oxiserial.aio` also has pyserial-asyncio's functions, so code written for
pyserial-asyncio changes only its import:

```python
import asyncio

from oxiserial import aio as serial_asyncio


async def main() -> None:
    reader, writer = await serial_asyncio.open_serial_connection(
        url="COM3", baudrate=115200
    )
    writer.write(b"*IDN?\n")
    await writer.drain()
    print(await reader.readline())
    writer.close()
    await writer.wait_closed()


asyncio.run(main())
```

`create_serial_connection` and `connection_for_serial` connect an asyncio
protocol to a port, as they do in pyserial-asyncio.

## Test without hardware

`serial_for_url("loop://")` opens a port that reads back what is written to
it. Other names open the device, as `Serial` does.

```python
from oxiserial import serial_for_url

port = serial_for_url("loop://", baudrate=115200, timeout=0.01)
port.write(b"ping--")
print(port.read_until(expected=b"--"))  # b'ping--'
port.close()
```

`oxiserial.aio.serial_for_url` does the same and returns an
`oxiserial.aio.Serial`.

## Find a port

```python
from oxiserial.tools.list_ports import comports

for port in sorted(comports()):
    print(port.device, port.description, port.hwid)
```

`grep(regexp)`, from the same module, returns only the ports whose device,
description or hwid matches `regexp`, ignoring case.

## Drive an RS-485 bus

An RS-485 transceiver sends only while RTS is raised, so a port that drives
one has to raise RTS around each write. The `rs485` setting does that on
`Serial` and `oxiserial.aio.Serial` alike:

```python
from oxiserial import RS485, Serial

port = Serial("/dev/ttyS1", 115200, rs485=True)
port = Serial("COM4", rs485=RS485(delay_after_tx=0.002))
port.rs485_switched_by  # "driver" or "oxiserial"
```

`rs485_switched_by` tells whether the OS driver switches RTS or oxiserial
does it around each write. When oxiserial does it, RTS drops once the OS
buffer is empty, which can be slightly before the UART has sent its last
bytes, so a device that needs RTS held to the end wants a `delay_after_tx`.

## Differences from pyserial

- `write()` also accepts `str` and sends it as UTF-8.
- `read_until()` takes `expected=None` as a newline and matches a `str`
  as UTF-8. pyserial raises `TypeError` for `None` and never matches a
  `str`.
- On Windows with `write_timeout=0`, a second write made while the first
  is still pending returns 0 and sends nothing, and `flush()` raises the
  error of a pending write that failed.
- pyserial's deprecated camelCase methods (`inWaiting()`, `setRTS()`, ...)
  are not provided; use the properties (`in_waiting`, `rts`, ...).
- `serial_for_url` accepts device names and `loop://`; other URLs, such as
  `socket://`, raise `ValueError`. `Serial("loop://")` also opens a
  loopback port.
- pyserial's `serial.rs485` module, with `RS485Settings`, the
  `rs485_mode` property and the `RS485` subclass of `Serial`, is replaced
  by the `rs485` setting; see [Drive an RS-485 bus](#drive-an-rs-485-bus).
- `serial.threaded` is `oxiserial.threaded`.

## License

BSD-3-Clause, see [LICENSE](LICENSE). Parts derived from pyserial carry its
notice in [LICENSES/pyserial.txt](LICENSES/pyserial.txt).
