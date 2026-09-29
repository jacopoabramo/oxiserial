from oxiserial.tools.list_ports import ListPortInfo, comports


def test_usb_info_matches_pyserial_format() -> None:
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
    assert all(isinstance(port, ListPortInfo) for port in comports())


def test_equality_compares_device_only() -> None:
    port = ListPortInfo("COM3")
    assert port == ListPortInfo("COM3")
    assert hash(port) == hash(ListPortInfo("COM3"))
    assert port != ListPortInfo("COM4")
    assert port != "COM3"
    assert port != 3
