# oxiserial

[![CI](https://github.com/jacopoabramo/oxiserial/actions/workflows/ci.yaml/badge.svg)](https://github.com/jacopoabramo/oxiserial/actions/workflows/ci.yaml)
[![License: BSD-3-Clause](https://img.shields.io/badge/license-BSD--3--Clause-blue.svg)](LICENSE)
[![Python 3.11+](https://img.shields.io/badge/python-3.11%2B-blue.svg)](https://www.python.org/downloads/)
[![Typed](https://img.shields.io/badge/typing-mypy%20strict-blue.svg)](https://mypy.readthedocs.io/)

Serial port access for Python, written in Rust. It has the API of
[pyserial](https://pypi.org/project/pyserial/) 3.5, and adds an asyncio
interface whose I/O calls return futures.

Runs on Windows, Linux and macOS with CPython 3.11 or newer, including the
free-threaded build.

## Install

oxiserial is not on PyPI yet. Building from source needs a Rust toolchain:

```sh
pip install git+https://github.com/jacopoabramo/oxiserial
```

## Replace pyserial

Change the import; the rest of the code stays as it is.

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

## Find a port

```python
from oxiserial.tools.list_ports import comports

for port in sorted(comports()):
    print(port.device, port.description, port.hwid)
```

## Differences from pyserial

- `write()` also accepts `str` and sends it as UTF-8.
- pyserial's deprecated camelCase methods (`inWaiting()`, `setRTS()`, ...)
  are not provided; use the properties (`in_waiting`, `rts`, ...).
- `serial_for_url`, `serial.threaded` and `serial.rs485` are not available
  yet.

## License

BSD-3-Clause, see [LICENSE](LICENSE). Parts derived from pyserial carry its
notice in [LICENSES/pyserial.txt](LICENSES/pyserial.txt).
