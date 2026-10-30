# Changelog

Changes a user of `oxiserial` notices, newest release first. Releases up to
0.3.0 are described in their
[GitHub releases](https://github.com/jacopoabramo/oxiserial/releases).

<!-- towncrier release notes start -->

## [0.4.0](https://github.com/jacopoabramo/oxiserial/releases/tag/v0.4.0) - 30-10-2026

### Added

- Add `oxiserial.to_bytes`, which converts its argument to `bytes` as pyserial's does. ([#16](https://github.com/jacopoabramo/oxiserial/pull/16))
- Add `oxiserial.serialutil`, with pyserial's constants, exceptions, `SerialBase` and `to_bytes`. ([#16](https://github.com/jacopoabramo/oxiserial/pull/16))
- Add `oxiserial.tools.list_ports_common`, with `ListPortInfo`. ([#16](https://github.com/jacopoabramo/oxiserial/pull/16))
- Add `oxiserial.tools.list_ports.grep`, which returns the ports whose device, description or hwid matches a regular expression, ignoring case. ([#16](https://github.com/jacopoabramo/oxiserial/pull/16))
- Add `oxiserial.threaded`, with pyserial's `Protocol`, `Packetizer`, `FramedPacket`, `LineReader` and `ReaderThread`, which is generic in its protocol type. ([#17](https://github.com/jacopoabramo/oxiserial/pull/17))
- Add wheels for free-threaded Python 3.15 and newer, built for the `abi3t` stable ABI. ([#20](https://github.com/jacopoabramo/oxiserial/pull/20))
- Add `RS485` and the `rs485` setting of `Serial` and `oxiserial.aio.Serial`, which raise RTS around each write for an RS-485 transceiver: `Serial(port, rs485=True)`. ([#21](https://github.com/jacopoabramo/oxiserial/pull/21))
- Add `Serial.rs485_switched_by`, which says whether the OS driver or oxiserial switches RTS. ([#21](https://github.com/jacopoabramo/oxiserial/pull/21))

### Changed

- On Windows, `Serial.flush` and `oxiserial.aio.Serial.flush` return within about 1 ms of the driver's output queue emptying, where they could take up to 16 ms. ([#24](https://github.com/jacopoabramo/oxiserial/pull/24))

### Fixed

- A child process made with `os.fork` while a port is open no longer panics on macOS. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `oxiserial.tools.list_ports.ListPortInfo` raises `IndexError` with pyserial's message for an index past 2. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `Serial` raises `SerialException` for a panic in the first 50 ms of a blocking call, as it does later in the call. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `oxiserial.tools.list_ports.comports` releases the GIL while it checks each Linux `ttyS` port for a UART. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `oxiserial.aio.Future.add_done_callback` runs a callback added outside an event loop off the I/O threads, so one that blocks no longer stalls every port. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `oxiserial.aio.SerialTransport.set_write_buffer_limits` computes its defaults as pyserial-asyncio does, and no longer overflows on large values. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `oxiserial.aio.SerialTransport` ends the connection with an error when a write or the closing flush cannot start. ([#15](https://github.com/jacopoabramo/oxiserial/pull/15))
- `oxiserial.tools.list_ports.comports` fills `description`, `hwid` and `manufacturer` for ports that are not USB, as pyserial does: from Device Manager on Windows, and for ACPI (`pnp`) and AMBA ports on Linux. ([#26](https://github.com/jacopoabramo/oxiserial/pull/26))
- `oxiserial.tools.list_ports.comports` on Windows lists a port whose device has a problem, such as a disabled driver, as pyserial does. ([#27](https://github.com/jacopoabramo/oxiserial/pull/27))
