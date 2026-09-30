"""Test-only mock ports and helpers."""

from oxiserial.aio import Future

__all__ = [
    "mock_pair",
    "mock_block_writes",
    "mock_unplug",
    "mock_state",
    "delayed",
    "panic_in_task",
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
    """Return the `baudrate`, `rts`, `dtr` and `break` state of a mock port.

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

def debug_build() -> bool:
    """Return whether the extension was compiled without optimisations."""
