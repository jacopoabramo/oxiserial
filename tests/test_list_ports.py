import os
import sys

import pytest

from oxiserial.tools.list_ports import ListPortInfo, comports


def test_usb_info_matches_pyserial_format() -> None:
    """Build the description and hardware id from the USB fields."""
    port = ListPortInfo("/dev/ttyUSB0")
    port.vid = 0x0403
    port.pid = 0x6001
    port.serial_number = "A1"
    port.location = "1-1.2"
    port.product = "FT232R"
    port.apply_usb_info()
    device, description, hwid = port
    assert (device, description, hwid) == (
        "/dev/ttyUSB0",
        "FT232R",
        "USB VID:PID=0403:6001 SER=A1 LOCATION=1-1.2",
    )
    assert port.name == "ttyUSB0"


def test_comports_returns_port_info() -> None:
    """Return only ListPortInfo objects from comports."""
    assert all(isinstance(port, ListPortInfo) for port in comports())


def test_equality_compares_device_only() -> None:
    """Compare and hash ports by device alone."""
    port = ListPortInfo("COM3")
    assert port == ListPortInfo("COM3")
    assert hash(port) == hash(ListPortInfo("COM3"))
    assert port != ListPortInfo("COM4")
    assert port != "COM3"
    assert port != 3


def test_sorted_orders_ports_naturally() -> None:
    """Sort ports so that COM2 comes before COM10."""
    ports = sorted([ListPortInfo("COM10"), ListPortInfo("COM2"), ListPortInfo("COM1")])
    assert [port.device for port in ports] == ["COM1", "COM2", "COM10"]


def test_index_past_hwid_raises_like_pyserial() -> None:
    """Raise IndexError with pyserial's message for an index past 2."""
    with pytest.raises(IndexError, match=r"^3 > 2$"):
        ListPortInfo("COM1")[3]


def test_ordering_against_other_types_raises() -> None:
    """Raise TypeError when ordering a port against another type."""
    with pytest.raises(TypeError):
        ListPortInfo("COM1") < 5  # type: ignore[operator]  # noqa: B015


def test_subclass_and_dynamic_attributes() -> None:
    """Allow ListPortInfo subclasses and new attributes on instances."""

    class Tagged(ListPortInfo):
        def tag(self) -> str:
            return f"tag:{self.name}"

    assert Tagged("/dev/ttyS0").tag() == "tag:ttyS0"
    port = ListPortInfo("COM1")
    port.custom = 1  # type: ignore[attr-defined]
    assert port.custom == 1  # type: ignore[attr-defined]


@pytest.mark.skipif(sys.platform != "linux", reason="reads Linux sysfs")
def test_comports_leaves_out_ports_without_a_uart() -> None:
    """Leave out ttyS ports whose sysfs UART type is 0, the unused legacy slots."""
    for port in comports():
        name = os.path.basename(port.device)
        try:
            with open(f"/sys/class/tty/{name}/type") as uart_type:
                assert uart_type.read().strip() != "0", port.device
        except FileNotFoundError:
            continue
