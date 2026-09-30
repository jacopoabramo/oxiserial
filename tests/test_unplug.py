import threading

import pytest

from conftest import Unpluggable
from oxiserial import PortNotOpenError, Serial, SerialException


def test_read_after_unplug_closes_the_port(unpluggable_port: Unpluggable) -> None:
    """Raise on the first read after an unplug, then report the port closed."""
    name, unplug = unpluggable_port
    port = Serial(name, timeout=1)
    try:
        unplug()
        with pytest.raises(SerialException) as info:
            port.read(1)
        assert not isinstance(info.value, PortNotOpenError)
        assert not port.is_open
        with pytest.raises(PortNotOpenError):
            port.read(1)
    finally:
        port.close()


def test_unplug_ends_a_blocked_read(unpluggable_port: Unpluggable) -> None:
    """End a read that waits without a timeout when its port is unplugged."""
    name, unplug = unpluggable_port
    port = Serial(name, timeout=None)
    try:
        threading.Timer(0.2, unplug).start()
        with pytest.raises(SerialException) as info:
            port.read(1)
        assert not isinstance(info.value, PortNotOpenError)
        assert not port.is_open
    finally:
        port.close()
