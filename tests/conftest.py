import asyncio
import errno
import os
import time
from collections.abc import Callable, Coroutine
from typing import Any

import pytest

import oxiserial

Runner = Callable[[Coroutine[Any, Any, Any]], Any]


def pytest_addoption(parser: pytest.Parser) -> None:
    """Add the options that name a connected pair of real ports."""
    parser.addoption("--port-a", default=os.environ.get("OXISERIAL_PORT_A"))
    parser.addoption("--port-b", default=os.environ.get("OXISERIAL_PORT_B"))


@pytest.fixture
def mock_pair() -> tuple[str, str]:
    """Provide the names of two connected mock ports."""
    testing = pytest.importorskip("oxiserial._testing")
    names: tuple[str, str] = testing.mock_pair()
    return names


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
        rsloop = pytest.importorskip("rsloop")
        runner: Runner = rsloop.run
        return runner
    return asyncio.run
