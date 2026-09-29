import asyncio
import os
from collections.abc import Callable, Coroutine
from typing import Any

import pytest

Runner = Callable[[Coroutine[Any, Any, Any]], Any]


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption("--port-a", default=os.environ.get("OXISERIAL_PORT_A"))
    parser.addoption("--port-b", default=os.environ.get("OXISERIAL_PORT_B"))


@pytest.fixture
def mock_pair() -> tuple[str, str]:
    testing = pytest.importorskip("oxiserial._testing")
    names: tuple[str, str] = testing.mock_pair()
    return names


@pytest.fixture
def real_pair(request: pytest.FixtureRequest) -> tuple[str, str]:
    port_a = request.config.getoption("--port-a")
    port_b = request.config.getoption("--port-b")
    if not port_a or not port_b:
        pytest.skip("no connected port pair given (--port-a/--port-b)")
    return str(port_a), str(port_b)


@pytest.fixture(params=["asyncio", "rsloop"])
def run(request: pytest.FixtureRequest) -> Runner:
    if request.param == "rsloop":
        rsloop = pytest.importorskip("rsloop")
        runner: Runner = rsloop.run
        return runner
    return asyncio.run
