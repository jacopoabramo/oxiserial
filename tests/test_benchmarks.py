import threading
from collections.abc import Callable
from pathlib import Path

import pytest
from benchmarks.run import (
    BENCHES,
    IntegrityError,
    cell,
    check,
    failure_cell,
    main,
    pattern,
    wait_for_ports,
)


def swap(data: bytes) -> bytes:
    """Swap the bytes at offsets 3 and 7, which differ in the pattern."""
    return data[:3] + data[7:8] + data[4:7] + data[3:4] + data[8:]


def drop(data: bytes) -> bytes:
    """Remove the byte at offset 7."""
    return data[:7] + data[8:]


def duplicate(data: bytes) -> bytes:
    """Repeat the byte at offset 7."""
    return data[:8] + data[7:]


@pytest.mark.parametrize(("damage", "offset"), [(swap, 3), (drop, 7), (duplicate, 8)])
def test_check_rejects_damaged_data(
    damage: Callable[[bytes], bytes], offset: int
) -> None:
    """Reject damaged data, naming the offset of the first wrong byte."""
    sent = pattern(64)
    with pytest.raises(IntegrityError, match=f"offset {offset};"):
        check("S1", sent, damage(sent))


def test_only_rejects_unknown_ids(capsys: pytest.CaptureFixture[str]) -> None:
    """Exit with a usage error naming the valid ids for an unknown --only id."""
    with pytest.raises(SystemExit) as info:
        main(["--only", "L9"])
    assert info.value.code == 2
    assert "L1" in capsys.readouterr().err


def test_wait_for_ports_waits_for_the_path_to_appear(tmp_path: Path) -> None:
    """Return only once an absolute port path exists, as a restarting socat link."""
    link = tmp_path / "ttyA"
    threading.Timer(0.2, link.touch).start()
    wait_for_ports([str(link)], timeout=5)
    assert link.exists()


@pytest.mark.parametrize(
    "argv",
    [["--only", ""], ["--only", " , "], ["--port-a", "COM3"], ["--port-b", "COM4"]],
)
def test_rejects_incomplete_options(
    argv: list[str],
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Exit with a usage error for an empty --only or a port option without its pair."""
    monkeypatch.delenv("OXISERIAL_PORT_A", raising=False)
    monkeypatch.delenv("OXISERIAL_PORT_B", raising=False)
    with pytest.raises(SystemExit) as info:
        main(argv)
    assert info.value.code == 2
    assert capsys.readouterr().err


def test_cell_shows_small_ratios() -> None:
    """Show a ratio far below 1 with its significant digits instead of 0.00x."""
    stream = next(bench for bench in BENCHES if bench.streaming)
    assert cell([0.001], stream, [1.0]).endswith(", 0.001x")


def test_failure_cell_keeps_the_table_intact() -> None:
    """Escape pipes and flatten newlines in a failure message."""
    text = failure_cell(ValueError("a|b\nc"))
    assert text == "FAILED: ValueError: a\\|b c"
