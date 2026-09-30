import asyncio
import errno
import os
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


def plug_in_port() -> Generator[Unpluggable, None, None]:
    """Provide a com0com plug-in mode port; unplugging closes its partner.

    `OXISERIAL_PLUGIN_PAIR` names the pair as `PORT,PARTNER`, where `PORT` was
    installed with `PlugInMode=yes` and exists only while `PARTNER` is open.
    """
    pair = os.environ.get("OXISERIAL_PLUGIN_PAIR")
    if not pair:
        pytest.skip("no com0com plug-in pair given (OXISERIAL_PLUGIN_PAIR)")
    name, partner_name = pair.split(",")
    partner = oxiserial.Serial(partner_name)
    try:
        wait_until(lambda: opens(name), f"{name} appearing")
        yield name, partner.close
    finally:
        partner.close()


@pytest.fixture
def unpluggable_port(tmp_path: Path) -> Generator[Unpluggable, None, None]:
    """Provide a port name and a function that unplugs that port, or skip."""
    if sys.platform == "win32":
        yield from plug_in_port()
    else:
        yield from socat_port(tmp_path)
