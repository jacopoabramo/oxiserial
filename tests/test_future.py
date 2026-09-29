import asyncio

import pytest

from conftest import Runner

pytest.importorskip("oxiserial._testing")
from oxiserial import _testing  # noqa: E402


def test_wait_returns_the_value() -> None:
    future = _testing.delayed(b"x", 0.05)
    assert not future.done()
    assert future.wait() == b"x"
    assert future.done()
    assert future.result() == b"x"


def test_await_and_wait_see_the_same_result(run: Runner) -> None:
    future = _testing.delayed(b"x", 0.05)

    async def main() -> list[bytes]:
        return list(await asyncio.gather(future, future))

    assert run(main()) == [b"x", b"x"]
    assert future.wait() == b"x"


def test_wait_timeout_leaves_the_operation_running() -> None:
    future = _testing.delayed(b"x", 0.3)
    with pytest.raises(TimeoutError):
        future.wait(timeout=0.01)
    assert future.wait() == b"x"


def test_result_before_completion_raises() -> None:
    future = _testing.delayed(b"x", 5)
    with pytest.raises(asyncio.InvalidStateError):
        future.result()
    future.cancel()


def test_cancel_ends_the_operation() -> None:
    future = _testing.delayed(b"x", 10)
    assert future.cancel()
    with pytest.raises(asyncio.CancelledError):
        future.wait()
    assert not future.cancel()


def test_cancelled_await_cancels_the_operation(run: Runner) -> None:
    future = _testing.delayed(b"x", 10)

    async def main() -> None:
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(future, 0.05)

    run(main())
    assert future.done()
    with pytest.raises(asyncio.CancelledError):
        future.result()


def test_completion_after_the_loop_closed() -> None:
    future = _testing.delayed(b"x", 0.1)

    async def waiter() -> bytes:
        return await future

    loop = asyncio.new_event_loop()
    loop.create_task(waiter())
    loop.run_until_complete(asyncio.sleep(0.01))
    loop.close()
    assert future.wait() == b"x"
