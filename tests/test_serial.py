import _thread
import array
import errno
import io
import sys
import threading
import time
from collections.abc import Iterator
from types import MappingProxyType
from typing import Any

import pytest

from oxiserial import (
    Baudrate,
    PortNotOpenError,
    Serial,
    SerialException,
    SerialTimeoutException,
    _testing,
)


@pytest.fixture
def ports(mock_pair: tuple[str, str]) -> Iterator[tuple[Serial, Serial]]:
    """Provide two connected open mock ports and close them afterwards."""
    a = Serial(mock_pair[0], timeout=1)
    b = Serial(mock_pair[1], timeout=1)
    yield a, b
    a.close()
    b.close()


def test_write_then_read(ports: tuple[Serial, Serial]) -> None:
    """Read back the bytes written to the peer port."""
    a, b = ports
    assert a.write(b"hello") == 5
    assert b.read(5) == b"hello"


def test_timeout_returns_partial_data(ports: tuple[Serial, Serial]) -> None:
    """Return partial data when the timeout expires before all bytes arrive."""
    a, b = ports
    b.timeout = 0.1
    a.write(b"ab")
    assert b.read(5) == b"ab"


def test_lines(ports: tuple[Serial, Serial]) -> None:
    """Read lines with read_until, readline, readlines and iteration."""
    a, b = ports
    a.write(b"one\ntwo\n")
    assert b.read_until() == b"one\n"
    assert b.readline() == b"two\n"
    b.timeout = 0.1
    a.write(b"1\n2\n")
    assert b.readlines() == [b"1\n", b"2\n"]
    a.write(b"3\n")
    assert list(b) == [b"3\n"]


def test_readinto(ports: tuple[Serial, Serial]) -> None:
    """Fill a buffer with the bytes read."""
    a, b = ports
    buffer = bytearray(4)
    a.write(b"abcd")
    assert b.readinto(buffer) == 4
    assert buffer == b"abcd"


def test_write_timeout(
    ports: tuple[Serial, Serial], mock_pair: tuple[str, str]
) -> None:
    """Raise SerialTimeoutException when a write cannot finish in time."""
    a, _ = ports
    _testing.mock_block_writes(mock_pair[0], True)
    a.write_timeout = 0.05
    with pytest.raises(SerialTimeoutException):
        a.write(b"x")


def test_modem_lines_cross_over(ports: tuple[Serial, Serial]) -> None:
    """Show the RTS and DTR lines of one port as inputs on the peer."""
    a, b = ports
    a.rts = False
    assert not b.cts
    a.rts = True
    assert b.cts
    a.dtr = False
    assert not b.dsr and not b.cd


def test_buffers(ports: tuple[Serial, Serial]) -> None:
    """Count bytes in the input buffer and empty it on reset."""
    a, b = ports
    a.write(b"abc")
    assert b.in_waiting == 3
    b.reset_input_buffer()
    assert b.in_waiting == 0


def test_settings_apply_to_an_open_port(
    ports: tuple[Serial, Serial], mock_pair: tuple[str, str]
) -> None:
    """Apply setting changes to a port that is open."""
    a, b = ports
    a.baudrate = 115200
    assert _testing.mock_state(mock_pair[0])["baudrate"] == 115200
    b.apply_settings(a.get_settings() | {"parity": "E"})
    assert b.baudrate == 115200 and b.parity == "E" and b.timeout == 1


def test_break(ports: tuple[Serial, Serial], mock_pair: tuple[str, str]) -> None:
    """Set and clear the break condition."""
    a, _ = ports
    a.send_break(0.01)
    assert _testing.mock_state(mock_pair[0])["break"] is False
    a.break_condition = True
    assert _testing.mock_state(mock_pair[0])["break"] is True


def test_open_close_lifecycle(mock_pair: tuple[str, str]) -> None:
    """Follow the open and close rules for a port from creation to close."""
    port = Serial()
    with pytest.raises(SerialException):
        port.open()
    port.port = mock_pair[0]
    with pytest.raises(PortNotOpenError):
        port.read()
    with port:
        assert port.is_open
        with pytest.raises(SerialException):
            port.open()
    assert not port.is_open
    port.open()
    port.close()


@pytest.mark.parametrize(
    "kwargs",
    [
        {"bytesize": 9},
        {"parity": "X"},
        {"stopbits": 3},
        {"timeout": -1},
        {"baudrate": -1},
    ],
)
def test_invalid_settings_raise_value_error(kwargs: dict[str, object]) -> None:
    """Raise ValueError for each invalid constructor setting."""
    with pytest.raises(ValueError):
        Serial(**kwargs)  # type: ignore[arg-type]


def test_ctrl_c_aborts_a_blocking_read(ports: tuple[Serial, Serial]) -> None:
    """Interrupt a blocking read with Ctrl-C and keep the port usable."""
    a, b = ports
    b.timeout = None
    threading.Timer(0.1, _thread.interrupt_main).start()
    with pytest.raises(KeyboardInterrupt):
        b.read(10)
    a.write(b"abc")
    b.timeout = 1
    assert b.read(3) == b"abc"


def test_close_from_another_thread_ends_a_blocked_read(
    ports: tuple[Serial, Serial],
) -> None:
    """Fail a blocked read when another thread closes the port."""
    _, b = ports
    b.timeout = None
    threading.Timer(0.1, b.close).start()
    with pytest.raises(PortNotOpenError):
        b.read(1)


def test_write_while_a_read_is_blocked(ports: tuple[Serial, Serial]) -> None:
    """Write from one thread while another is blocked reading."""
    a, b = ports
    results: list[bytes] = []
    reader = threading.Thread(target=lambda: results.append(b.read(3)))
    reader.start()
    time.sleep(0.05)
    b.write(b"zz")
    assert a.read(2) == b"zz"
    a.write(b"abc")
    reader.join()
    assert results == [b"abc"]


def test_write_accepts_bytes_like_and_str(ports: tuple[Serial, Serial]) -> None:
    """Accept bytes-like objects and str in write, sending str as UTF-8."""
    a, b = ports
    a.write(bytearray(b"a"))
    a.write(memoryview(b"b"))
    assert b.read(2) == b"ab"
    assert a.write("h\xe9") == 3
    assert b.read(3) == b"h\xc3\xa9"
    with pytest.raises(UnicodeEncodeError):
        a.write("\ud800")


def test_write_accepts_buffers_and_rejects_ints(ports: tuple[Serial, Serial]) -> None:
    """Send the raw bytes of any buffer and reject ints and iterables of ints."""
    a, b = ports
    assert a.write(array.array("H", [1])) == 2
    assert a.write(memoryview(b"abcd").cast("I")) == 4
    assert b.read(6) == b"\x01\x00abcd"
    for data in (5, [2, 3], (2, 3), iter([2, 3]), object()):
        with pytest.raises(TypeError, match=type(data).__name__):
            a.write(data)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="list"):
        b.read_until([10])  # type: ignore[arg-type]
    assert b.in_waiting == 0


def test_flush_returns_after_a_write(ports: tuple[Serial, Serial]) -> None:
    """Return from flush once written data is transmitted."""
    a, b = ports
    a.write(b"abc")
    a.flush()
    assert b.read(3) == b"abc"


def test_subclass_can_add_parameters_and_attributes(mock_pair: tuple[str, str]) -> None:
    """Support subclasses with extra constructor parameters and attributes."""

    class MySerial(Serial):
        def __init__(self, port: str, extra: str, **kwargs: Any) -> None:
            super().__init__(port, **kwargs)
            self.extra = extra

    a = MySerial(mock_pair[0], "x", timeout=1)
    b = Serial(mock_pair[1], timeout=1)
    try:
        assert a.is_open and a.extra == "x"
        a.write(b"hi")
        assert b.read(2) == b"hi"
    finally:
        a.close()
        b.close()


def test_numeric_settings_are_coerced_like_pyserial() -> None:
    """Coerce numeric settings like pyserial and reject non-numeric ones."""
    port = Serial(baudrate=9600.0, bytesize=8.0)  # type: ignore[arg-type]
    assert port.baudrate == 9600 and port.bytesize == 8
    port.baudrate = "19200"  # type: ignore[assignment]
    assert port.baudrate == 19200
    with pytest.raises(ValueError, match="baudrate"):
        Serial(baudrate="fast")  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="byte size"):
        Serial(bytesize=8.5)  # type: ignore[arg-type]


def test_timeouts_read_back_as_the_number_given() -> None:
    """Keep each timeout as the int or float it was set to, as pyserial does."""
    port = Serial(timeout=1, write_timeout=0.5)
    port.apply_settings({"inter_byte_timeout": 2})
    assert [type(port.timeout), type(port.write_timeout)] == [int, float]
    assert type(port.get_settings()["inter_byte_timeout"]) is int
    assert "timeout=1," in repr(port)
    with pytest.raises(ValueError, match="Not a valid timeout: 'soon'"):
        port.timeout = "soon"  # type: ignore[assignment]


def test_dsrdtr_none_follows_rtscts() -> None:
    """Follow rtscts when dsrdtr is None."""
    assert Serial(rtscts=True, dsrdtr=None).dsrdtr is True
    assert Serial(rtscts=True).dsrdtr is False
    port = Serial(rtscts=True)
    port.dsrdtr = None
    assert port.dsrdtr is True


def test_apply_settings_accepts_any_mapping() -> None:
    """Accept any mapping in apply_settings and leave other settings alone."""
    port = Serial()
    port.apply_settings(MappingProxyType({"baudrate": 4800, "timeout": 2}))
    assert port.baudrate == 4800 and port.timeout == 2
    port.apply_settings(MappingProxyType({"timeout": None}))
    assert port.timeout is None and port.baudrate == 4800


def test_exit_accepts_any_arguments(mock_pair: tuple[str, str]) -> None:
    """Close the port from __exit__ whatever arguments it receives."""
    port = Serial(mock_pair[0])
    port.__exit__()
    assert not port.is_open


@pytest.mark.parametrize("duration", [-1, float("nan")])
def test_send_break_rejects_invalid_durations(
    ports: tuple[Serial, Serial], duration: float
) -> None:
    """Reject negative and NaN break durations without starting a break."""
    a, _ = ports
    with pytest.raises(ValueError):
        a.send_break(duration)
    assert not a.break_condition


def test_open_and_close_go_through_overridable_methods(
    mock_pair: tuple[str, str],
) -> None:
    """Call the overridden open and close methods from the context manager."""
    calls: list[str] = []

    class Recording(Serial):
        def open(self) -> None:
            calls.append("open")
            super().open()

        def close(self) -> None:
            calls.append("close")
            super().close()

    with Recording(mock_pair[0]):
        pass
    port = Recording()
    port.port = mock_pair[1]
    with port:
        assert port.is_open
    assert calls == ["open", "close", "open", "close"]


def test_flags_accept_any_truthy_value(mock_pair: tuple[str, str]) -> None:
    """Accept any truthy or falsy value for the boolean settings."""
    port = Serial(mock_pair[0], rtscts=0, xonxoff=1)  # type: ignore[arg-type]
    try:
        assert port.rtscts is False and port.xonxoff is True and port.dsrdtr is False
        port.dtr = 0  # type: ignore[assignment]
        assert port.dtr is False
        assert _testing.mock_state(mock_pair[0])["dtr"] is False
    finally:
        port.close()


@pytest.mark.skipif(sys.platform == "win32", reason="the missing path is POSIX only")
def test_opening_a_missing_port_reports_the_errno() -> None:
    """Report the OS error number when the device does not exist."""
    with pytest.raises(SerialException) as info:
        Serial("/dev/oxiserial-does-not-exist")
    assert info.value.errno is not None


def test_opening_a_missing_port_reports_the_windows_error() -> None:
    """Report the Windows error code once, with its errno, for a missing port."""
    if sys.platform != "win32":
        pytest.skip("the Windows error code is Windows only")
    with pytest.raises(SerialException) as info:
        Serial("COM250")
    assert info.value.winerror == 2
    assert info.value.errno == errno.ENOENT
    assert "(os error" not in str(info.value)


def test_read_all_portstr_and_repr(
    ports: tuple[Serial, Serial], mock_pair: tuple[str, str]
) -> None:
    """Read all buffered bytes and show the port in portstr and repr."""
    a, b = ports
    a.write(b"xyz")
    assert b.in_waiting == 3
    assert b.read_all() == b"xyz"
    assert a.portstr == mock_pair[0]
    assert repr(a) == (
        f"Serial<id=0x{id(a):x}, open=True>(port={mock_pair[0]!r}, baudrate=9600, "
        "bytesize=8, parity='N', stopbits=1, timeout=1, xonxoff=False, "
        "rtscts=False, dsrdtr=False)"
    )


def test_baudrate_enum_and_nonstandard_rates(mock_pair: tuple[str, str]) -> None:
    """Accept Baudrate members and rates outside the standard list."""
    port = Serial(mock_pair[0], Baudrate.B115200)
    try:
        assert port.baudrate == 115200
        port.baudrate = 250000
        assert _testing.mock_state(mock_pair[0])["baudrate"] == 250000
        assert Serial.BAUDRATES == tuple(Baudrate)
    finally:
        port.close()


def test_read_until_accepts_bytes_like(ports: tuple[Serial, Serial]) -> None:
    """Accept any bytes-like terminator in read_until."""
    a, b = ports
    a.write(b"ab;cd;")
    assert b.read_until(bytearray(b";")) == b"ab;"
    assert b.read_until(memoryview(b";")) == b"cd;"


def test_text_io_over_a_port(ports: tuple[Serial, Serial]) -> None:
    """Read and write lines through io.TextIOWrapper over a port."""
    a, b = ports
    b.timeout = 0.1
    text = io.TextIOWrapper(io.BufferedRWPair(b, b), newline="\n")  # type: ignore[type-var]
    a.write(b"hello\n")
    assert text.readline() == "hello\n"
    text.write("bye\n")
    text.flush()
    assert a.readline() == b"bye\n"
    assert not b.closed
    text.close()
    assert b.closed
