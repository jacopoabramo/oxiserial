from typing import Final

from oxiserial import aio as aio, tools as tools

__version__: str

PARITY_NONE: Final = "N"
PARITY_EVEN: Final = "E"
PARITY_ODD: Final = "O"
PARITY_MARK: Final = "M"
PARITY_SPACE: Final = "S"
PARITY_NAMES: dict[str, str]
STOPBITS_ONE: Final = 1
STOPBITS_ONE_POINT_FIVE: Final = 1.5
STOPBITS_TWO: Final = 2
FIVEBITS: Final = 5
SIXBITS: Final = 6
SEVENBITS: Final = 7
EIGHTBITS: Final = 8
XON: Final = b"\x11"
XOFF: Final = b"\x13"
CR: Final = b"\r"
LF: Final = b"\n"

class SerialException(OSError): ...
class SerialTimeoutException(SerialException): ...
class PortNotOpenError(SerialException): ...
