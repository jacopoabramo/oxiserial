"""Serial port whose I/O methods return futures."""

from collections.abc import Callable, Generator
from contextvars import Context
from types import GenericAlias
from typing import Any, Generic, Self, TypeVar, final

from typing_extensions import Buffer

from oxiserial import SerialBase

__all__ = ["Future", "Serial"]

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
