"""Serial port whose I/O methods return futures."""

import asyncio
from collections.abc import Callable, Coroutine, Generator, Iterable
from contextvars import Context
from types import GenericAlias
from typing import Any, Generic, Self, TypeVar, final

from typing_extensions import Buffer

from oxiserial import SerialBase

__all__ = [
    "Future",
    "Serial",
    "SerialTransport",
    "connection_for_serial",
    "create_serial_connection",
    "open_serial_connection",
    "serial_for_url",
]

_T_co = TypeVar("_T_co", covariant=True)

@final
class Future(Generic[_T_co]):
    """Result of a port operation that runs in the background.

    Block on it with [`wait`][oxiserial.aio.Future.wait] or await it inside a
    running event loop. Awaiting it and then cancelling the awaiting task
    cancels the operation.
    """

    def wait(self, timeout: float | None = None) -> _T_co:
        """Block until the operation finishes and return its result.

        `None` waits without limit. Ctrl-C interrupts the wait.

        Raises
        ------
        TimeoutError
            If `timeout` seconds pass first. The operation keeps running.
        asyncio.CancelledError
            If the operation was cancelled.
        SerialException
            If the operation failed. Subclasses such as
            [`PortNotOpenError`][oxiserial.PortNotOpenError] and
            [`SerialTimeoutException`][oxiserial.SerialTimeoutException] are
            raised as they are.
        """
    def done(self) -> bool:
        """Return `True` if the operation has finished, failed or been cancelled."""
    def cancel(self) -> bool:
        """Cancel the operation if it is still running.

        Return `True` if this call cancelled it and `False` if it had already
        finished. Bytes a cancelled read had already collected are discarded;
        the ones it had not taken stay in the input buffer.
        """
    def cancelled(self) -> bool:
        """Return `True` if the operation was cancelled."""
    def exception(self) -> BaseException | None:
        """Return the exception the operation failed with, or `None` on success.

        Every call returns the same object, and it is the one that
        [`result`][oxiserial.aio.Future.result] and
        [`wait`][oxiserial.aio.Future.wait] raise.

        Raises
        ------
        asyncio.CancelledError
            If the operation was cancelled.
        asyncio.InvalidStateError
            If the operation has not finished.
        """
    def add_done_callback(
        self, fn: Callable[[Self], object], /, *, context: Context | None = None
    ) -> None:
        """Call `fn` with this future once the operation has finished.

        If an event loop is running in the calling thread, `fn` runs on that
        loop, as with `asyncio.Future`. Otherwise it runs in the thread that
        finishes the operation, as with `concurrent.futures.Future`. If the
        operation has already finished, `fn` is scheduled on the running loop,
        or called at once when no loop is running.

        An exception raised by `fn` goes to the loop's exception handler, or to
        `sys.unraisablehook` when no loop is running; the remaining callbacks
        still run. If the loop `fn` belongs to has closed before the operation
        finishes, `fn` is not called and the `RuntimeError` from scheduling it
        goes to `sys.unraisablehook`.

        Parameters
        ----------
        context
            The context `fn` runs in. `None` uses a copy of the current context.
        """
    def remove_done_callback(self, fn: Callable[[Self], object], /) -> int:
        """Remove every registration of `fn` and return how many were removed.

        Callbacks are compared with `==`.
        """
    def result(self) -> _T_co:
        """Return the result of a finished operation without waiting.

        Raises
        ------
        asyncio.InvalidStateError
            If the operation has not finished.
        asyncio.CancelledError
            If the operation was cancelled.
        SerialException
            If the operation failed, including its subclasses as in
            [`wait`][oxiserial.aio.Future.wait].
        """
    def __await__(self) -> Generator[Any, None, _T_co]:
        """Wait for the result inside the running event loop.

        Raises
        ------
        RuntimeError
            If no event loop is running.
        """
    def __class_getitem__(cls, item: Any, /) -> GenericAlias: ...

class Serial(SerialBase):
    """Serial port whose I/O methods return a [`Future`][oxiserial.aio.Future].

    Each method behaves like the method of the same name on
    [`oxiserial.Serial`][oxiserial.Serial] and produces its result through the
    future. Call `wait()` on the future to block, or await it. Settings,
    modem lines, `open` and `close` are inherited and are not futures. Use it
    as an async context manager to close the port on exit.
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
    def read(self, size: int = 1) -> Future[bytes]:
        """Read up to `size` bytes.

        See [`Serial.read`][oxiserial.Serial.read]. Errors such as
        [`PortNotOpenError`][oxiserial.PortNotOpenError] and `SerialException`
        for a disconnected device are raised through the future.
        """
    def read_until(
        self, expected: Buffer | str | None = b"\n", size: int | None = None
    ) -> Future[bytes]:
        """Read until `expected` or `size` bytes.

        See [`Serial.read_until`][oxiserial.Serial.read_until].

        Raises
        ------
        TypeError
            If `expected` is not `bytes`, `str` or an object with the buffer
            protocol. This is raised by the call, not through the future.
        """
    def readline(self, size: int = -1) -> Future[bytes]:
        """Read one line.

        See [`Serial.readline`][oxiserial.Serial.readline].
        """
    def readlines(self, hint: int = -1) -> Future[list[bytes]]:
        """Read lines until a read times out empty.

        See [`Serial.readlines`][oxiserial.Serial.readlines].
        """
    def write(self, data: Buffer | str) -> Future[int]:
        """Write `data`; the future yields the number of bytes written.

        See [`Serial.write`][oxiserial.Serial.write]. A closed port and an
        expired `write_timeout` are reported through the future.

        Raises
        ------
        TypeError
            If `data` is not `bytes`, `str` or an object with the buffer
            protocol. This is raised by the call, not through the future.
        """
    def flush(self) -> Future[None]:
        """Wait until written data is transmitted.

        See [`Serial.flush`][oxiserial.Serial.flush].
        """
    def send_break(self, duration: float = 0.25) -> Future[None]:
        """Hold a break condition for `duration` seconds.

        See [`Serial.send_break`][oxiserial.Serial.send_break].
        """
    def read_all(self) -> Future[bytes]:
        """Read the bytes currently in the input buffer.

        Raises
        ------
        PortNotOpenError
            If the port is closed. This is raised by the call, not through the
            future.
        """
    def __aenter__(self) -> Future[Self]:
        """Open the port if a port is set and it is closed.

        The port opens during this call and the future yields it.

        Raises
        ------
        SerialException
            If the device cannot be opened. This is raised by the call, not
            through the future.
        """
    def __aexit__(self, *args: object) -> Future[None]:
        """Close the port."""

def serial_for_url(
    url: str | None, *args: Any, do_not_open: bool = False, **kwargs: Any
) -> Serial:
    """Create a [`Serial`][oxiserial.aio.Serial] for a device name or URL and open it.

    Takes the arguments of [`oxiserial.serial_for_url`][oxiserial.serial_for_url],
    including `loop://`.

    Raises
    ------
    ValueError
        If `url` has a scheme other than `loop://`, or a setting is not valid.
    SerialException
        If the device cannot be opened.
    """

_P = TypeVar("_P", bound=asyncio.BaseProtocol)

@final
class SerialTransport:
    """An asyncio transport over a serial port, as in pyserial-asyncio.

    Received bytes reach the protocol's `data_received` in order; `write`
    queues bytes without blocking and calls the protocol's `pause_writing`
    and `resume_writing` around the write-buffer limits. It is not a subclass
    of `asyncio.Transport`, but has its methods.
    """

    @property
    def loop(self) -> asyncio.AbstractEventLoop:
        """The event loop the transport runs on."""
    @property
    def serial(self) -> SerialBase:
        """The port, the same object as `get_extra_info("serial")`."""
    def get_extra_info(self, name: str, default: Any = None) -> Any:
        """Return the port for `"serial"`, and `default` for any other name."""
    def is_closing(self) -> bool: ...
    def is_reading(self) -> bool: ...
    def close(self) -> None:
        """Stop reading, send the queued bytes, then close the port.

        The protocol's `connection_lost(None)` is called once the port is
        closed.
        """
    def abort(self) -> None:
        """Close at once, dropping the queued bytes."""
    def write(self, data: Buffer | str) -> None:
        """Queue `data` to be sent; ignored once the transport is closing."""
    def writelines(self, list_of_data: Iterable[Buffer | str]) -> None: ...
    def can_write_eof(self) -> bool:
        """Return False: serial ports have no end-of-file."""
    def write_eof(self) -> None:
        """Raise, as serial ports have no end-of-file.

        Raises
        ------
        NotImplementedError
            Always.
        """
    def pause_reading(self) -> None: ...
    def resume_reading(self) -> None: ...
    def set_write_buffer_limits(
        self, high: int | None = None, low: int | None = None
    ) -> None:
        """Set the write-buffer limits for `pause_writing` and `resume_writing`.

        Raises
        ------
        ValueError
            Unless `high >= low >= 0`.
        """
    def get_write_buffer_limits(self) -> tuple[int, int]: ...
    def get_write_buffer_size(self) -> int:
        """Return the bytes queued or being written."""
    def flush(self) -> None:
        """Discard the queued bytes, as pyserial-asyncio does."""
    def get_protocol(self) -> asyncio.BaseProtocol | None: ...
    def set_protocol(self, protocol: asyncio.BaseProtocol) -> None: ...

def create_serial_connection(
    loop: asyncio.AbstractEventLoop,
    protocol_factory: Callable[[], _P],
    url: str | None,
    *args: Any,
    **kwargs: Any,
) -> Coroutine[Any, Any, tuple[SerialTransport, _P]]:
    """Open `url` and connect it to a new protocol.

    `url` and the other arguments go to
    [`oxiserial.serial_for_url`][oxiserial.serial_for_url].

    Raises
    ------
    ValueError
        If `url` has an unknown scheme or a setting is not valid.
    SerialException
        If the device cannot be opened.
    """

def connection_for_serial(
    loop: asyncio.AbstractEventLoop,
    protocol_factory: Callable[[], _P],
    serial_instance: SerialBase,
) -> Coroutine[Any, Any, tuple[SerialTransport, _P]]:
    """Connect an open port to a new protocol."""

def open_serial_connection(
    *,
    loop: asyncio.AbstractEventLoop | None = None,
    limit: int | None = None,
    **kwargs: Any,
) -> Coroutine[Any, Any, tuple[asyncio.StreamReader, asyncio.StreamWriter]]:
    """Open a port and return a `StreamReader` and `StreamWriter` for it.

    The keyword arguments go to
    [`oxiserial.serial_for_url`][oxiserial.serial_for_url]; `limit` is the
    reader's buffer limit.

    Raises
    ------
    ValueError
        If `url` has an unknown scheme or a setting is not valid.
    SerialException
        If the device cannot be opened.
    """
