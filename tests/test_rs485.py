import math
import threading
import time
from typing import Any

import pytest

from oxiserial import (
    RS485,
    PortNotOpenError,
    Serial,
    SerialBase,
    SerialTimeoutException,
    _testing,
)
from oxiserial.aio import Serial as AioSerial


def test_rs485_defaults_and_repr() -> None:
    """Default to RTS high while sending, low otherwise, no delays and no loopback."""
    value = RS485()
    assert (value.rts_while_tx, value.rts_while_rx, value.loopback) == (
        True,
        False,
        False,
    )
    assert (value.delay_before_tx, value.delay_after_tx) == (0.0, 0.0)
    assert repr(RS485(delay_after_tx=0.002)) == (
        "RS485(rts_while_tx=True, rts_while_rx=False, delay_before_tx=0.0, "
        "delay_after_tx=0.002, loopback=False)"
    )


def test_rs485_compares_and_hashes_by_value() -> None:
    """Compare and hash RS485 values by their fields."""
    assert RS485(delay_before_tx=0.001) == RS485(delay_before_tx=0.001)
    assert RS485() != RS485(loopback=True)
    assert len({RS485(), RS485()}) == 1


def test_rs485_takes_keywords_only() -> None:
    """Refuse positional arguments."""
    with pytest.raises(TypeError):
        RS485(True)  # type: ignore[call-arg]


@pytest.mark.parametrize("delay", [-0.001, math.inf, math.nan])
@pytest.mark.parametrize("field", ["delay_before_tx", "delay_after_tx"])
def test_rs485_rejects_bad_delays(field: str, delay: float) -> None:
    """Raise ValueError naming the field for a negative, infinite or NaN delay."""
    kwargs: dict[str, Any] = {field: delay}
    with pytest.raises(ValueError, match=field):
        RS485(**kwargs)


@pytest.mark.parametrize("cls", [Serial, AioSerial])
def test_rs485_setting_round_trips(
    mock_pair: tuple[str, str], cls: type[SerialBase]
) -> None:
    """Store True as RS485(), an RS485 as given and False as off, on both classes."""
    port = cls(mock_pair[0], rs485=True)  # type: ignore[call-arg]
    assert port.rs485 == RS485()
    assert port.rs485_switched_by == "oxiserial"
    port.rs485 = RS485(delay_after_tx=0.002)
    assert port.rs485 == RS485(delay_after_tx=0.002)
    port.rs485 = False
    assert port.rs485 is None
    assert port.rs485_switched_by is None
    port.close()


def test_rs485_switched_by_is_none_when_closed(mock_pair: tuple[str, str]) -> None:
    """Report no switching on a closed port, and switching again after a reopen."""
    port = Serial(rs485=True)
    assert port.rs485_switched_by is None
    port.port = mock_pair[0]
    port.open()
    assert port.rs485_switched_by == "oxiserial"
    port.port = mock_pair[1]
    assert port.rs485_switched_by == "oxiserial"
    port.close()


@pytest.mark.parametrize("value", [1, "yes"])
def test_rs485_rejects_other_types(value: object) -> None:
    """Raise TypeError for anything but a bool, an RS485 or None."""
    with pytest.raises(TypeError, match="rs485"):
        Serial(rs485=value)  # type: ignore[arg-type]


def test_rs485_and_rtscts_conflict(mock_pair: tuple[str, str]) -> None:
    """Raise ValueError when RS-485 and RTS/CTS flow control are both on."""
    with pytest.raises(ValueError, match="rtscts"):
        Serial(mock_pair[0], rtscts=True, rs485=True)
    port = Serial(mock_pair[0], rs485=True)
    with pytest.raises(ValueError, match="rtscts"):
        port.rtscts = True
    assert port.rtscts is False
    port.close()


def test_loopback_without_a_driver_raises(mock_pair: tuple[str, str]) -> None:
    """Raise ValueError for loopback where oxiserial switches RTS."""
    with pytest.raises(ValueError, match="loopback"):
        Serial(mock_pair[0], rs485=RS485(loopback=True))
    port = Serial(mock_pair[0], rs485=True)
    with pytest.raises(ValueError, match="loopback"):
        port.rs485 = RS485(loopback=True)
    assert port.rs485 == RS485()
    assert port.rs485_switched_by == "oxiserial"
    port.close()


def test_rts_set_during_rs485_applies_when_it_turns_off(
    mock_pair: tuple[str, str],
) -> None:
    """Hold RTS at the receive level while RS-485 is on, then apply a stored level."""
    port = Serial(mock_pair[0], rs485=True)
    assert _testing.mock_state(mock_pair[0])["rts"] is False
    port.rts = True
    assert _testing.mock_state(mock_pair[0])["rts"] is False
    port.rs485 = False
    assert _testing.mock_state(mock_pair[0])["rts"] is True
    port.close()


@pytest.mark.parametrize("cls", [Serial, AioSerial])
def test_a_write_switches_rts_around_the_data(
    mock_pair: tuple[str, str], cls: type[SerialBase]
) -> None:
    """Raise RTS, wait, send, wait and drop RTS, in order, on both classes."""
    rs485 = RS485(delay_before_tx=0.02, delay_after_tx=0.03)
    port = cls(mock_pair[0], rs485=rs485)  # type: ignore[call-arg]
    _testing.mock_take_events(mock_pair[0])
    result = port.write(b"abc")  # type: ignore[attr-defined]
    assert (result.wait(2) if cls is AioSerial else result) == 3
    events = _testing.mock_take_events(mock_pair[0])
    assert [(kind, value) for _, kind, value in events] == [
        ("rts", 1),
        ("write", 3),
        ("rts", 0),
    ]
    (up, _, _), (sent, _, _), (down, _, _) = events
    assert sent - up >= 0.02
    assert down - sent >= 0.03
    port.close()


def test_a_timed_out_write_drops_rts(mock_pair: tuple[str, str]) -> None:
    """Leave RTS at the receive level when a switched write times out."""
    port = Serial(mock_pair[0], write_timeout=0.05, rs485=True)
    _testing.mock_block_writes(mock_pair[0], True)
    with pytest.raises(SerialTimeoutException):
        port.write(b"abc")
    assert _testing.mock_state(mock_pair[0])["rts"] is False
    port.close()


def test_a_cancelled_async_write_drops_rts(mock_pair: tuple[str, str]) -> None:
    """Leave RTS at the receive level when a switched async write is cancelled."""
    port = AioSerial(mock_pair[0], rs485=True)
    _testing.mock_block_writes(mock_pair[0], True)
    future = port.write(b"abc")
    time.sleep(0.05)
    assert _testing.mock_state(mock_pair[0])["rts"] is True
    future.cancel()
    deadline = time.monotonic() + 2
    while _testing.mock_state(mock_pair[0])["rts"] and time.monotonic() < deadline:
        time.sleep(0.01)
    assert _testing.mock_state(mock_pair[0])["rts"] is False
    port.close()


def test_closing_during_a_switched_write_ends_it(mock_pair: tuple[str, str]) -> None:
    """End a blocked switched write with PortNotOpenError when the port closes."""
    port = Serial(mock_pair[0], rs485=True)
    _testing.mock_block_writes(mock_pair[0], True)
    closer = threading.Timer(0.05, port.close)
    closer.start()
    with pytest.raises(PortNotOpenError):
        port.write(b"abc")
    closer.join()


def test_a_non_blocking_switched_write_still_drains(
    mock_pair: tuple[str, str],
) -> None:
    """Drop RTS only after the data with write_timeout=0."""
    port = Serial(mock_pair[0], write_timeout=0, rs485=True)
    _testing.mock_take_events(mock_pair[0])
    assert port.write(b"abc") == 3
    kinds = [kind for _, kind, _ in _testing.mock_take_events(mock_pair[0])]
    assert kinds == ["rts", "write", "rts"]
    port.close()
