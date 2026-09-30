import threading
from collections.abc import Callable
from pathlib import Path

import pytest
from benchmarks.run import IntegrityError, check, main, pattern, wait_for_ports


def swap(data: bytes) -> bytes:
    """Swap the bytes at offsets 3 and 7, which differ in the pattern."""
    return data[:3] + data[7:8] + data[4:7] + data[3:4] + data[8:]


def drop(data: bytes) -> bytes:
    """Remove the byte at offset 7."""
    return data[:7] + data[8:]


def duplicate(data: bytes) -> bytes:
    """Repeat the byte at offset 7."""
    return data[:8] + data[7:]


@pytest.mark.parametrize("damage", [swap, drop, duplicate])
def test_check_rejects_damaged_data(damage: Callable[[bytes], bytes]) -> None:
    """Reject received data with a swapped, dropped or duplicated byte."""
    sent = pattern(64)
    with pytest.raises(IntegrityError, match="offset"):
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
