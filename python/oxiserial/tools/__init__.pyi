"""Serial port discovery."""

from oxiserial.tools import list_ports as list_ports
from oxiserial.tools import list_ports_common as list_ports_common

__all__ = ["list_ports", "list_ports_common"]
