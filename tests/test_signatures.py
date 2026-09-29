import inspect
from typing import Any

import pytest

import oxiserial
import oxiserial.aio


@pytest.mark.parametrize("cls", [oxiserial.Serial, oxiserial.aio.Serial])
def test_method_signatures_are_introspectable(cls: Any) -> None:
    """Expose readable signatures on the read methods of both classes."""
    read_until = inspect.signature(cls.read_until)
    assert read_until.parameters["expected"].default == b"\n"
    inspect.signature(cls.readline)
    inspect.signature(cls.readlines)
