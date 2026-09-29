import importlib


def test_submodules_import_by_dotted_name() -> None:
    """Import each submodule by its dotted name."""
    for name in ("oxiserial.aio", "oxiserial.tools", "oxiserial.tools.list_ports"):
        assert importlib.import_module(name).__name__ == name
