"""The names pyserial code imports from `serial.serialutil`."""

from oxiserial import CR as CR
from oxiserial import EIGHTBITS as EIGHTBITS
from oxiserial import FIVEBITS as FIVEBITS
from oxiserial import LF as LF
from oxiserial import PARITY_EVEN as PARITY_EVEN
from oxiserial import PARITY_MARK as PARITY_MARK
from oxiserial import PARITY_NAMES as PARITY_NAMES
from oxiserial import PARITY_NONE as PARITY_NONE
from oxiserial import PARITY_ODD as PARITY_ODD
from oxiserial import PARITY_SPACE as PARITY_SPACE
from oxiserial import SEVENBITS as SEVENBITS
from oxiserial import SIXBITS as SIXBITS
from oxiserial import STOPBITS_ONE as STOPBITS_ONE
from oxiserial import STOPBITS_ONE_POINT_FIVE as STOPBITS_ONE_POINT_FIVE
from oxiserial import STOPBITS_TWO as STOPBITS_TWO
from oxiserial import XOFF as XOFF
from oxiserial import XON as XON
from oxiserial import PortNotOpenError as PortNotOpenError
from oxiserial import SerialBase as SerialBase
from oxiserial import SerialException as SerialException
from oxiserial import SerialTimeoutException as SerialTimeoutException
from oxiserial import to_bytes as to_bytes

__all__ = [
    "XON",
    "XOFF",
    "CR",
    "LF",
    "PARITY_NONE",
    "PARITY_EVEN",
    "PARITY_ODD",
    "PARITY_MARK",
    "PARITY_SPACE",
    "PARITY_NAMES",
    "STOPBITS_ONE",
    "STOPBITS_ONE_POINT_FIVE",
    "STOPBITS_TWO",
    "FIVEBITS",
    "SIXBITS",
    "SEVENBITS",
    "EIGHTBITS",
    "SerialException",
    "SerialTimeoutException",
    "PortNotOpenError",
    "SerialBase",
    "to_bytes",
]
