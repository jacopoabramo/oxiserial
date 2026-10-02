import array
import sys
from typing import Any

import pytest
from serial.serialutil import to_bytes as serial_to_bytes  # type: ignore[attr-defined]

import oxiserial
from oxiserial import serialutil

MISSING_PORT = "COM250" if sys.platform == "win32" else "/dev/oxiserial-does-not-exist"


@pytest.mark.parametrize(
    "seq",
    [
        b"ab",
        bytearray(b"ab"),
        memoryview(b"ab"),
        array.array("H", [1, 2]),
        [97, 98],
        (0, 255),
        3,
    ],
)
def test_to_bytes_matches_pyserial(seq: Any) -> None:
    """Convert bytes-like objects, sequences of ints and sizes as pyserial does."""
    assert oxiserial.to_bytes(seq) == serial_to_bytes(seq)


def test_to_bytes_rejects_str_like_pyserial() -> None:
    """Raise TypeError with pyserial's message for a str."""
    with pytest.raises(TypeError) as ours:
        oxiserial.to_bytes("ab")  # type: ignore[arg-type]
    with pytest.raises(TypeError) as theirs:
        serial_to_bytes("ab")
    assert str(ours.value) == str(theirs.value)


def test_serialutil_exception_catches_a_failed_open() -> None:
    """Catch a failed open with the exception imported from serialutil."""
    with pytest.raises(serialutil.SerialException):
        oxiserial.Serial(MISSING_PORT)
