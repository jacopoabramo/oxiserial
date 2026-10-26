import math
from typing import Any

import pytest

from oxiserial import RS485


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
