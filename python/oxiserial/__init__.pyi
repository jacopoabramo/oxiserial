"""Serial port access with a pyserial-compatible API."""

from collections.abc import Iterator, Mapping
from enum import IntEnum
from typing import Any, ClassVar, Final, Self

from typing_extensions import Buffer, disjoint_base

from oxiserial import aio as aio
from oxiserial import tools as tools

__version__: str

PARITY_NONE: Final = "N"
PARITY_EVEN: Final = "E"
PARITY_ODD: Final = "O"
PARITY_MARK: Final = "M"
PARITY_SPACE: Final = "S"
PARITY_NAMES: dict[str, str]
STOPBITS_ONE: Final = 1
STOPBITS_ONE_POINT_FIVE: Final = 1.5
STOPBITS_TWO: Final = 2
FIVEBITS: Final = 5
SIXBITS: Final = 6
SEVENBITS: Final = 7
EIGHTBITS: Final = 8
XON: Final = b"\x11"
XOFF: Final = b"\x13"
CR: Final = b"\r"
LF: Final = b"\n"

class Baudrate(IntEnum):
    """Standard baud rates; members compare equal to their integer value."""

    B50 = 50
    B75 = 75
    B110 = 110
    B134 = 134
    B150 = 150
    B200 = 200
    B300 = 300
    B600 = 600
    B1200 = 1200
    B1800 = 1800
    B2400 = 2400
    B4800 = 4800
    B9600 = 9600
    B19200 = 19200
    B38400 = 38400
    B57600 = 57600
    B115200 = 115200
    B230400 = 230400
    B460800 = 460800
    B500000 = 500000
    B576000 = 576000
    B921600 = 921600
    B1000000 = 1000000
    B1152000 = 1152000
    B1500000 = 1500000
    B2000000 = 2000000
    B2500000 = 2500000
    B3000000 = 3000000
    B3500000 = 3500000
    B4000000 = 4000000

class SerialException(OSError):
    """Base class for serial port errors."""

class SerialTimeoutException(SerialException):
    """Raised when a write does not finish within `write_timeout`."""

class PortNotOpenError(SerialException):
    """Raised when an operation needs an open port and the port is closed."""

@disjoint_base
class SerialBase:
    """Settings, modem lines and buffer control shared by the blocking and async ports.

    Assigning a setting on an open port reconfigures the port at once, except
    `exclusive`, which applies at the next open. An invalid value raises
    `ValueError`.
    """

    BAUDRATES: ClassVar[tuple[Baudrate, ...]]
    BYTESIZES: ClassVar[tuple[int, ...]]
    PARITIES: ClassVar[tuple[str, ...]]
    STOPBITS: ClassVar[tuple[float, ...]]
    @property
    def port(self) -> str | None:
        """Device name, or `None` if none is set.

        Assigning to an open port closes it and reopens it on the new device.
        """
    @port.setter
    def port(self, value: str | None) -> None: ...
    @property
    def name(self) -> str | None:
        """Same as `port`."""
    @property
    def portstr(self) -> str | None:
        """Same as `port`."""
    @property
    def is_open(self) -> bool:
        """Whether the port is open.

        A read or write that fails because the device is gone closes the port,
        so this turns `False` and later calls raise
        [`PortNotOpenError`][oxiserial.PortNotOpenError].
        """
    @property
    def baudrate(self) -> int:
        """Line speed in baud."""
    @baudrate.setter
    def baudrate(self, value: int) -> None: ...
    @property
    def bytesize(self) -> int:
        """Data bits per character: 5, 6, 7 or 8."""
    @bytesize.setter
    def bytesize(self, value: int) -> None: ...
    @property
    def parity(self) -> str:
        """Parity mode, one of the `PARITY_*` constants."""
    @parity.setter
    def parity(self, value: str) -> None: ...
    @property
    def stopbits(self) -> float:
        """Stop bits: 1, 1.5 or 2."""
    @stopbits.setter
    def stopbits(self, value: float) -> None: ...
    @property
    def timeout(self) -> float | None:
        """Read timeout in seconds.

        `None` waits until all requested bytes arrive, `0` returns what is
        already buffered, and a positive number returns what arrived in time.
        """
    @timeout.setter
    def timeout(self, value: float | None) -> None: ...
    @property
    def write_timeout(self) -> float | None:
        """Write timeout in seconds.

        `None` waits until all data is written. `0` returns the number of bytes
        accepted without waiting. A positive number raises
        [`SerialTimeoutException`][oxiserial.SerialTimeoutException] when the
        data is not written in time.
        """
    @write_timeout.setter
    def write_timeout(self, value: float | None) -> None: ...
    @property
    def inter_byte_timeout(self) -> float | None:
        """Longest gap in seconds between two bytes of a read before it ends.

        The limit starts once the first byte has arrived. `None` sets no limit.
        [`readline`][oxiserial.Serial.readline], `readlines` and iteration ignore
        it and apply `timeout` to each byte instead.
        """
    @inter_byte_timeout.setter
    def inter_byte_timeout(self, value: float | None) -> None: ...
    @property
    def xonxoff(self) -> bool:
        """Whether software flow control is enabled."""
    @xonxoff.setter
    def xonxoff(self, value: bool) -> None: ...
    @property
    def rtscts(self) -> bool:
        """Whether RTS/CTS hardware flow control is enabled."""
    @rtscts.setter
    def rtscts(self, value: bool) -> None: ...
    @property
    def dsrdtr(self) -> bool:
        """Whether DSR/DTR hardware flow control is enabled.

        Assigning `None` follows `rtscts`.
        """
    @dsrdtr.setter
    def dsrdtr(self, value: bool | None) -> None: ...
    @property
    def exclusive(self) -> bool | None:
        """Whether the port is opened for exclusive access.

        `None` uses the platform default. A change applies at the next open.
        """
    @exclusive.setter
    def exclusive(self, value: bool | None) -> None: ...
    @property
    def rts(self) -> bool:
        """State of the RTS line.

        While `rtscts` is on, flow control drives the line: an assigned level is
        stored and applied when `rtscts` is turned off.
        """
    @rts.setter
    def rts(self, value: bool) -> None: ...
    @property
    def dtr(self) -> bool:
        """State of the DTR line.

        On Windows, while `dsrdtr` is on, flow control drives the line: an
        assigned level is stored and applied when `dsrdtr` is turned off. Other
        platforms have no DSR/DTR flow control and set the line at once.
        """
    @dtr.setter
    def dtr(self, value: bool) -> None: ...
    @property
    def break_condition(self) -> bool:
        """Whether a break condition is held on the transmit line."""
    @break_condition.setter
    def break_condition(self, value: bool) -> None: ...
    @property
    def cts(self) -> bool:
        """State of the CTS line.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    @property
    def dsr(self) -> bool:
        """State of the DSR line.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    @property
    def ri(self) -> bool:
        """State of the RI line.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    @property
    def cd(self) -> bool:
        """State of the CD line.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    @property
    def in_waiting(self) -> int:
        """Number of bytes in the input buffer.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    @property
    def out_waiting(self) -> int:
        """Number of bytes in the output buffer.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    def open(self) -> None:
        """Open the port and discard pending input.

        Raises
        ------
        SerialException
            If no port is set, the port is already open, or the device cannot be opened.
        """
    def close(self) -> None:
        """Close the port; does nothing if it is already closed.

        Reads and writes waiting on the port fail with
        [`PortNotOpenError`][oxiserial.PortNotOpenError].
        """
    def reset_input_buffer(self) -> None:
        """Discard the bytes in the input buffer.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    def reset_output_buffer(self) -> None:
        """Discard the bytes in the output buffer.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    def get_settings(self) -> dict[str, Any]:
        """Return the current settings as a dictionary that `apply_settings` accepts."""
    def apply_settings(self, d: Mapping[str, Any]) -> None:
        """Apply the settings in `d`; settings it does not name are left unchanged.

        Raises
        ------
        ValueError
            If a value is not valid.
        """
    def fileno(self) -> int:
        """Return the file descriptor of the open port.

        Raises
        ------
        io.UnsupportedOperation
            If the platform has no file descriptor for the port.
        PortNotOpenError
            If the port is closed.
        """

class Serial(SerialBase):
    """Serial port with blocking I/O.

    The constructor opens the port when `port` is given. As a context manager
    the port is closed on exit; iterating over it yields lines.

    Notes
    -----
    [`io.TextIOWrapper`](https://docs.python.org/3/library/io.html#io.TextIOWrapper)
    over `io.BufferedRWPair(ser, ser)` uses
    [`readable`][oxiserial.Serial.readable],
    [`writable`][oxiserial.Serial.writable],
    [`closed`][oxiserial.Serial.closed],
    [`readinto`][oxiserial.Serial.readinto], [`write`][oxiserial.Serial.write]
    and [`close`][oxiserial.SerialBase.close], and calls `tell` if the port has
    it. Over `io.BufferedReader(ser)` or `io.BufferedWriter(ser)` it also calls
    [`seekable`][oxiserial.Serial.seekable], and `io.BufferedReader` calls
    [`flush`][oxiserial.Serial.flush].
    """

    def __init__(
        self,
        port: str | None = None,
        baudrate: int = 9600,
        bytesize: int = 8,
        parity: str = "N",
        stopbits: float = 1.0,
        timeout: float | None = None,
        xonxoff: bool = False,
        rtscts: bool = False,
        write_timeout: float | None = None,
        dsrdtr: bool | None = False,
        inter_byte_timeout: float | None = None,
        exclusive: bool | None = None,
    ) -> None:
        """Create the port, and open it if `port` is given.

        Raises
        ------
        ValueError
            If a setting is not valid.
        SerialException
            If `port` is given and the device cannot be opened.
        """
    def read(self, size: int = 1) -> bytes:
        """Read up to `size` bytes.

        The result is shorter than `size` when `timeout` or
        `inter_byte_timeout` ends the read first.

        Raises
        ------
        PortNotOpenError
            If the port is closed, also while the read waits.
        SerialException
            If the device is disconnected.
        """
    def read_until(
        self, expected: Buffer | str | None = b"\n", size: int | None = None
    ) -> bytes:
        """Read until `expected` arrives, `size` bytes are read or the read times out.

        The result includes `expected`; `None` stands for a newline. `timeout`
        covers the whole call.

        Raises
        ------
        TypeError
            If `expected` is not `bytes`, `str` or an object with the buffer
            protocol.
        PortNotOpenError
            If the port is closed, also while the read waits.
        SerialException
            If the device is disconnected.
        """
    def readline(self, size: int = -1) -> bytes:
        """Read one line, ending at a newline or after `size` bytes.

        A negative `size` sets no limit. `timeout` applies to each byte, so a
        line that keeps arriving is not cut short. `inter_byte_timeout` has no
        effect.

        Raises
        ------
        PortNotOpenError
            If the port is closed, also while the read waits.
        SerialException
            If the device is disconnected.
        """
    def readlines(self, hint: int = -1) -> list[bytes]:
        """Read lines until one read times out with no data.

        With a positive `hint`, stop once at least that many bytes are collected.

        Raises
        ------
        PortNotOpenError
            If the port is closed, also while the read waits.
        """
    @property
    def closed(self) -> bool:
        """`True` when the port is not open.

        For compatibility with
        [`io.TextIOWrapper`](https://docs.python.org/3/library/io.html#io.TextIOWrapper).
        A read or write that fails because the device is gone closes the port,
        so this turns `True`.
        """
    def readable(self) -> bool:
        """Return `True`.

        For compatibility with
        [`io.TextIOWrapper`](https://docs.python.org/3/library/io.html#io.TextIOWrapper).
        """
    def writable(self) -> bool:
        """Return `True`.

        For compatibility with
        [`io.TextIOWrapper`](https://docs.python.org/3/library/io.html#io.TextIOWrapper).
        """
    def seekable(self) -> bool:
        """Return `False`.

        For compatibility with
        [`io.TextIOWrapper`](https://docs.python.org/3/library/io.html#io.TextIOWrapper).
        """
    def readinto(self, b: Buffer) -> int:
        """Read into the writable buffer `b` and return the number of bytes stored.

        Raises
        ------
        PortNotOpenError
            If the port is closed, also while the read waits.
        """
    def write(self, data: Buffer | str) -> int:
        """Write `data` and return the number of bytes written.

        A `str` is sent as UTF-8; an object with the buffer protocol, such as
        `bytearray`, `memoryview` or `array.array`, is sent as its raw bytes.

        Raises
        ------
        TypeError
            If `data` is not `bytes`, `str` or an object with the buffer
            protocol, for example an `int` or a list of ints.
        PortNotOpenError
            If the port is closed.
        SerialTimeoutException
            If `write_timeout` is a positive number and expires first.
        """
    def flush(self) -> None:
        """Wait until all written data has been transmitted.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    def send_break(self, duration: float = 0.25) -> None:
        """Hold the transmit line in a break condition for `duration` seconds.

        Raises
        ------
        ValueError
            If `duration` is negative.
        PortNotOpenError
            If the port is closed.
        """
    def read_all(self) -> bytes:
        """Read the bytes currently in the input buffer.

        Raises
        ------
        PortNotOpenError
            If the port is closed.
        """
    def __enter__(self) -> Self:
        """Open the port if a port is set and it is closed, and return it."""
    def __exit__(self, *args: object) -> None:
        """Close the port."""
    def __iter__(self) -> Iterator[bytes]:
        """Return the port itself; iteration yields lines."""
    def __next__(self) -> bytes:
        """Read the next line, as [`readline`][oxiserial.Serial.readline] does.

        Raises
        ------
        StopIteration
            If the read returns no data.
        """

def serial_for_url(
    url: str | None, *args: Any, do_not_open: bool = False, **kwargs: Any
) -> Serial:
    """Create a [`Serial`][oxiserial.Serial] for a device name or URL and open it.

    `url` is a device name such as `COM3` or `/dev/ttyUSB0`, or `loop://`, a
    port with no hardware behind it: reads return the bytes written to it,
    `cts` follows `rts` and `dsr` follows `dtr`. The other arguments go to
    [`Serial`][oxiserial.Serial]. With `do_not_open` the port is returned
    closed.

    Raises
    ------
    ValueError
        If `url` has a scheme other than `loop://`, or a setting is not valid.
    SerialException
        If the device cannot be opened.
    """
