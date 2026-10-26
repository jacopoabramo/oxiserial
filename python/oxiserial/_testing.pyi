"""Test-only mock ports and helpers."""

from typing import Literal

from oxiserial.aio import Future

__all__ = [
    "mock_pair",
    "mock_block_writes",
    "mock_unplug",
    "mock_state",
    "mock_driver_rs485",
    "mock_fail_next",
    "mock_take_events",
    "delayed",
    "panic_in_task",
    "panic_in_call",
    "debug_build",
]

def mock_pair() -> tuple[str, str]:
    """Create two connected mock ports and return their names.

    Bytes written to one port are read from the other.
    """

def mock_block_writes(port: str, blocked: bool) -> None:
    """Make writes to the mock port `port` accept no bytes while `blocked` is true.

    Raises
    ------
    ValueError
        If `port` is not a mock port.
    """

def mock_unplug(port: str) -> None:
    """Make the mock port `port` fail reads and writes as an unplugged device does.

    Raises
    ------
    ValueError
        If `port` is not a mock port.
    """

def mock_state(port: str) -> dict[str, int | bool]:
    """Return the `baudrate`, `rts`, `dtr`, `break` and `rs485_on` state of a mock port.

    `rs485_on` is whether the driver's RS-485 mode is on.

    Raises
    ------
    ValueError
        If `port` is not a mock port.
    """

def mock_driver_rs485(port: str, enabled: bool) -> None:
    """Give the mock port `port` a driver that switches RTS when `rs485` is set.

    Raises
    ------
    ValueError
        If `port` is not a mock port.
    """

def mock_fail_next(port: str, operation: Literal["configure", "clear_buffers"]) -> None:
    """Make the next call of `operation` on the mock port `port` fail with an OS error.

    Raises
    ------
    ValueError
        If `port` is not a mock port, or `operation` is not one of the two.
    """

def mock_take_events(port: str) -> list[tuple[float, str, int]]:
    """Return and clear the RTS changes and writes of a mock port.

    Each entry is the seconds since the first entry, `"rts"` or `"write"`,
    and the new RTS level or the number of bytes written.

    Raises
    ------
    ValueError
        If `port` is not a mock port.
    """

def delayed(value: bytes, delay: float) -> Future[bytes]:
    """Return a future that yields `value` after `delay` seconds.

    Raises
    ------
    ValueError
        If `delay` is negative or not finite.
    """

def panic_in_task() -> Future[bytes]:
    """Return a future whose task panics, so waiting on it raises `SerialException`."""

def panic_in_call() -> None:
    """Make a blocking call that panics at once, which raises `SerialException`."""

def debug_build() -> bool:
    """Return whether the extension was compiled without optimisations."""
