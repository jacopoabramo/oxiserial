import _thread
import array
import threading
import time
from collections.abc import Iterator
from types import MappingProxyType
from typing import Any

import pytest

from oxiserial import PortNotOpenError, Serial, SerialException, SerialTimeoutException

pytest.importorskip("oxiserial._testing")
from oxiserial import _testing  # noqa: E402


@pytest.fixture
def ports(mock_pair: tuple[str, str]) -> Iterator[tuple[Serial, Serial]]:
    a = Serial(mock_pair[0], timeout=1)
    b = Serial(mock_pair[1], timeout=1)
    yield a, b
    a.close()
    b.close()


def test_write_then_read(ports: tuple[Serial, Serial]) -> None:
    a, b = ports
    assert a.write(b"hello") == 5
    assert b.read(5) == b"hello"


def test_timeout_returns_partial_data(ports: tuple[Serial, Serial]) -> None:
    a, b = ports
    b.timeout = 0.1
    a.write(b"ab")
    assert b.read(5) == b"ab"


def test_lines(ports: tuple[Serial, Serial]) -> None:
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
    a, b = ports
    buffer = bytearray(4)
    a.write(b"abcd")
    assert b.readinto(buffer) == 4
    assert buffer == b"abcd"


def test_write_timeout(ports: tuple[Serial, Serial], mock_pair: tuple[str, str]) -> None:
    a, _ = ports
    _testing.mock_block_writes(mock_pair[0], True)
    a.write_timeout = 0.05
    with pytest.raises(SerialTimeoutException):
        a.write(b"x")


def test_modem_lines_cross_over(ports: tuple[Serial, Serial]) -> None:
    a, b = ports
    a.rts = False
    assert not b.cts
    a.rts = True
    assert b.cts
    a.dtr = False
    assert not b.dsr and not b.cd


def test_buffers(ports: tuple[Serial, Serial]) -> None:
    a, b = ports
    a.write(b"abc")
    assert b.in_waiting == 3
    b.reset_input_buffer()
    assert b.in_waiting == 0


def test_settings_apply_to_an_open_port(
    ports: tuple[Serial, Serial], mock_pair: tuple[str, str]
) -> None:
    a, b = ports
    a.baudrate = 115200
    assert _testing.mock_state(mock_pair[0])["baudrate"] == 115200
    b.apply_settings(a.get_settings() | {"parity": "E"})
    assert b.baudrate == 115200 and b.parity == "E" and b.timeout == 1


def test_break(ports: tuple[Serial, Serial], mock_pair: tuple[str, str]) -> None:
    a, _ = ports
    a.send_break(0.01)
    assert _testing.mock_state(mock_pair[0])["break"] is False
    a.break_condition = True
    assert _testing.mock_state(mock_pair[0])["break"] is True


def test_open_close_lifecycle(mock_pair: tuple[str, str]) -> None:
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
    [{"bytesize": 9}, {"parity": "X"}, {"stopbits": 3}, {"timeout": -1}, {"baudrate": -1}],
)
def test_invalid_settings_raise_value_error(kwargs: dict[str, object]) -> None:
    with pytest.raises(ValueError):
        Serial(**kwargs)  # type: ignore[arg-type]


def test_ctrl_c_aborts_a_blocking_read(ports: tuple[Serial, Serial]) -> None:
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
    _, b = ports
    b.timeout = None
    threading.Timer(0.1, b.close).start()
    with pytest.raises(PortNotOpenError):
        b.read(1)


def test_write_while_a_read_is_blocked(ports: tuple[Serial, Serial]) -> None:
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
    a, b = ports
    a.write(bytearray(b"a"))
    a.write(memoryview(b"b"))
    assert b.read(2) == b"ab"
    assert a.write("h\xe9") == 3
    assert b.read(3) == b"h\xc3\xa9"
    with pytest.raises(UnicodeEncodeError):
        a.write("\ud800")


def test_write_accepts_whatever_bytearray_accepts(ports: tuple[Serial, Serial]) -> None:
    a, b = ports
    assert a.write([2, 3]) == 2  # type: ignore[arg-type]
    assert a.write(array.array("H", [1])) == 2
    assert a.write(memoryview(b"abcd").cast("I")) == 4
    assert a.write(5) == 5  # type: ignore[arg-type]
    assert b.read(13) == b"\x02\x03\x01\x00abcd" + b"\x00" * 5
    with pytest.raises(TypeError):
        a.write(object())  # type: ignore[arg-type]


def test_flush_returns_after_a_write(ports: tuple[Serial, Serial]) -> None:
    a, b = ports
    a.write(b"abc")
    a.flush()
    assert b.read(3) == b"abc"


def test_subclass_can_add_parameters_and_attributes(mock_pair: tuple[str, str]) -> None:
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
    port = Serial(baudrate=9600.0, bytesize=8.0)  # type: ignore[arg-type]
    assert port.baudrate == 9600 and port.bytesize == 8
    port.baudrate = "19200"  # type: ignore[assignment]
    assert port.baudrate == 19200
    with pytest.raises(ValueError, match="baudrate"):
        Serial(baudrate="fast")  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="byte size"):
        Serial(bytesize=8.5)  # type: ignore[arg-type]


def test_dsrdtr_none_follows_rtscts() -> None:
    assert Serial(rtscts=True, dsrdtr=None).dsrdtr is True
    assert Serial(rtscts=True).dsrdtr is False
    port = Serial(rtscts=True)
    port.dsrdtr = None
    assert port.dsrdtr is True


def test_apply_settings_accepts_any_mapping() -> None:
    port = Serial()
    port.apply_settings(MappingProxyType({"baudrate": 4800, "timeout": 2}))
    assert port.baudrate == 4800 and port.timeout == 2
    port.apply_settings(MappingProxyType({"timeout": None}))
    assert port.timeout is None and port.baudrate == 4800


def test_exit_accepts_any_arguments(mock_pair: tuple[str, str]) -> None:
    port = Serial(mock_pair[0])
    port.__exit__()
    assert not port.is_open


@pytest.mark.parametrize("duration", [-1, float("nan")])
def test_send_break_rejects_invalid_durations(
    ports: tuple[Serial, Serial], duration: float
) -> None:
    a, _ = ports
    with pytest.raises(ValueError):
        a.send_break(duration)
    assert not a.break_condition
