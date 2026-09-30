import asyncio
import errno
import os
import re
import shutil
import subprocess
import sys
import time
from collections.abc import Callable, Coroutine, Generator
from pathlib import Path
from typing import Any

import pytest
import rsloop

import oxiserial
from oxiserial import _testing

Runner = Callable[[Coroutine[Any, Any, Any]], Any]
Unpluggable = tuple[str, Callable[[], None]]


def pytest_addoption(parser: pytest.Parser) -> None:
    """Add the options that name a connected pair of real ports."""
    parser.addoption("--port-a", default=os.environ.get("OXISERIAL_PORT_A"))
    parser.addoption("--port-b", default=os.environ.get("OXISERIAL_PORT_B"))


@pytest.fixture
def mock_pair() -> tuple[str, str]:
    """Provide the names of two connected mock ports."""
    return _testing.mock_pair()


@pytest.fixture
def real_pair(request: pytest.FixtureRequest) -> tuple[str, str]:
    """Provide the connected real ports given on the command line, or skip."""
    port_a = request.config.getoption("--port-a")
    port_b = request.config.getoption("--port-b")
    if not port_a or not port_b:
        pytest.skip("no connected port pair given (--port-a/--port-b)")
    names = str(port_a), str(port_b)
    deadline = time.monotonic() + 5
    while any(os.path.isabs(n) and not os.path.exists(n) for n in names):
        if time.monotonic() > deadline:
            break
        time.sleep(0.05)
    return names


@pytest.fixture
def modem_pair(real_pair: tuple[str, str]) -> tuple[str, str]:
    """Provide the real pair if it has modem control lines, or skip."""
    with oxiserial.Serial(real_pair[1]) as port:
        try:
            _ = port.cts
        except oxiserial.SerialException as err:
            if err.errno in (errno.ENOTTY, errno.EINVAL):
                pytest.skip("the port pair has no modem control lines")
            raise
    return real_pair


@pytest.fixture(params=["asyncio", "rsloop"])
def run(request: pytest.FixtureRequest) -> Runner:
    """Provide a function that runs a coroutine on each supported event loop."""
    if request.param == "rsloop":
        runner: Runner = rsloop.run
        return runner
    return asyncio.run


def wait_until(ready: Callable[[], bool], what: str) -> None:
    """Poll `ready` for up to 5 seconds, failing the test if it never holds."""
    deadline = time.monotonic() + 5
    while not ready():
        if time.monotonic() > deadline:
            pytest.fail(f"{what} did not happen within 5 s")
        time.sleep(0.05)


def socat_port(tmp_path: Path) -> Generator[Unpluggable, None, None]:
    """Provide one end of a socat pty pair; unplugging kills socat."""
    socat = shutil.which("socat")
    if socat is None:
        pytest.skip("socat is not installed")
    a, b = tmp_path / "a", tmp_path / "b"
    process = subprocess.Popen(
        [socat, f"pty,raw,echo=0,link={a}", f"pty,raw,echo=0,link={b}"]
    )

    def unplug() -> None:
        process.kill()
        process.wait()

    try:
        wait_until(lambda: a.exists() and b.exists(), "socat creating its pty links")
        yield str(a), unplug
    finally:
        unplug()


def opens(name: str) -> bool:
    """Return whether the port `name` can be opened right now."""
    try:
        oxiserial.Serial(name).close()
    except oxiserial.SerialException:
        return False
    return True


def setupc(directory: str, *args: str) -> str:
    """Run com0com's setupc.exe from `directory` and return its output."""
    result = subprocess.run(
        [os.path.join(directory, "setupc.exe"), "--silent", *args],
        cwd=directory,
        capture_output=True,
        text=True,
        check=False,
    )
    output = result.stdout + result.stderr
    print(f"setupc {' '.join(args)} -> exit {result.returncode}\n{output}")
    return output


def com0com_port() -> Generator[Unpluggable, None, None]:
    """Provide a port of a new com0com pair; unplugging removes the pair.

    Needs `COM0COM_DIR` pointing at the directory holding setupc.exe, and
    administrator rights.
    """
    directory = os.environ.get("COM0COM_DIR")
    if not directory:
        pytest.skip("com0com is not available (COM0COM_DIR)")
    name = "COM94"
    output = setupc(directory, "install", f"PortName={name}", "PortName=COM95")
    match = re.search(rf"CNCA(\d+) PortName={name}", output)
    if match is None:
        pytest.skip(f"setupc did not create {name}")
    pair = match.group(1)
    removed = False

    def unplug() -> None:
        nonlocal removed
        removed = True
        setupc(directory, "remove", pair)

    try:
        wait_until(lambda: opens(name), f"{name} appearing")
        yield name, unplug
    finally:
        if not removed:
            setupc(directory, "remove", pair)


@pytest.fixture
def unpluggable_port(tmp_path: Path) -> Generator[Unpluggable, None, None]:
    """Provide a port name and a function that unplugs that port, or skip."""
    if sys.platform == "win32":
        yield from com0com_port()
    else:
        yield from socat_port(tmp_path)
