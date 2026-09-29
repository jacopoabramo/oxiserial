"""Serial port whose I/O methods return futures."""

from collections.abc import Generator
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
        """
    def done(self) -> bool:
        """Return `True` if the operation has finished, failed or been cancelled."""
    def cancel(self) -> bool:
        """Cancel the operation if it is still running.

        Return `True` if this call cancelled it and `False` if it had already
        finished. Bytes a cancelled read has not yet taken stay in the input
        buffer.
        """
    def result(self) -> _T_co:
        """Return the result of a finished operation without waiting.

        Raises
        ------
        asyncio.InvalidStateError
            If the operation has not finished.
        asyncio.CancelledError
            If the operation was cancelled.
        """
    def __await__(self) -> Generator[Any, None, _T_co]:
        """Wait for the result inside the running event loop."""
    def __class_getitem__(cls, key: Any) -> Any: ...

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

        See [`Serial.read`][oxiserial.Serial.read].
        """
    def read_until(
        self, expected: Buffer = b"\n", size: int | None = None
    ) -> Future[bytes]:
        """Read until `expected` or `size` bytes.

        See [`Serial.read_until`][oxiserial.Serial.read_until].
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
        """Read the bytes currently in the input buffer."""
    def __aenter__(self) -> Future[Self]:
        """Open the port if a port is set and it is closed.

        The future yields the port.
        """
    def __aexit__(self, *args: object) -> Future[None]:
        """Close the port."""
