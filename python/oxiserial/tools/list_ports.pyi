"""Enumeration of the serial ports on the system."""

from collections.abc import Iterator
from typing import Self

from typing_extensions import disjoint_base

__all__ = ["ListPortInfo", "comports"]

@disjoint_base
class ListPortInfo:
    """Description of one serial port.

    Unpacking or indexing yields `(device, description, hwid)`. Instances
    compare equal by `device` and sort in natural order, so `COM2` comes
    before `COM10`.
    """

    device: str
    name: str
    description: str
    hwid: str
    vid: int | None
    pid: int | None
    serial_number: str | None
    location: str | None
    manufacturer: str | None
    product: str | None
    interface: str | None
    def __new__(cls, device: str, skip_link_detection: bool = False) -> Self:
        """Describe `device`; `name` is its last path component.

        `description` and `hwid` start as `"n/a"` and the USB fields as `None`.
        `skip_link_detection` is accepted and has no effect.
        """
    def usb_description(self) -> str:
        """Return a description from `product` and `interface`, or from `name`."""
    def usb_info(self) -> str:
        """Return a hardware id from `vid`, `pid`, `serial_number` and `location`."""
    def apply_usb_info(self) -> None:
        """Set `description` and `hwid` from the USB fields."""
    def __getitem__(self, index: int, /) -> str:
        """Return `device`, `description` or `hwid` for index 0, 1 or 2.

        Raises
        ------
        IndexError
            If `index` is outside 0 to 2.
        """
    def __iter__(self) -> Iterator[str]:
        """Iterate over `device`, `description` and `hwid`."""
    def __lt__(self, other: ListPortInfo, /) -> bool:
        """Compare `device` in natural order.

        Raises
        ------
        TypeError
            If `other` is not a `ListPortInfo`.
        """
    def __eq__(self, other: object, /) -> bool:
        """Return `True` if `other` is a `ListPortInfo` with the same `device`."""
    def __hash__(self) -> int: ...

def comports(include_links: bool = False) -> list[ListPortInfo]:
    """Return the serial ports the system reports.

    `vid`, `pid`, `serial_number`, `manufacturer`, `product` and `location`
    are filled for USB ports, and `description` and `hwid` are built from them.
    Other ports keep `"n/a"` for both. `interface` is always `None`.
    `include_links` is accepted and has no effect.

    Raises
    ------
    SerialException
        If the ports cannot be listed.
    """
