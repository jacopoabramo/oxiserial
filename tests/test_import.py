import importlib
import sys
import sysconfig

import pytest

import oxiserial


def test_submodules_import_by_dotted_name() -> None:
    """Import each submodule by its dotted name."""
    for name in ("oxiserial.aio", "oxiserial.tools", "oxiserial.tools.list_ports"):
        assert importlib.import_module(name).__name__ == name


@pytest.mark.skipif(
    not sysconfig.get_config_var("Py_GIL_DISABLED"),
    reason="needs a free-threaded build",
)
def test_import_keeps_the_gil_disabled() -> None:
    """Keep the GIL disabled once oxiserial is imported."""
    assert oxiserial.__version__
    assert sys.version_info >= (3, 13)
    assert not sys._is_gil_enabled()
