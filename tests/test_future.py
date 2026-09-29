import asyncio
import contextvars
import sys
import threading
from typing import Any

import pytest

from conftest import Runner
from oxiserial import SerialException

pytest.importorskip("oxiserial._testing")
from oxiserial import _testing  # noqa: E402


def test_wait_returns_the_value() -> None:
    """Return the value from wait once the operation finishes."""
    future = _testing.delayed(b"x", 0.05)
    assert not future.done()
    assert future.wait() == b"x"
    assert future.done()
    assert future.result() == b"x"
    assert _testing.delayed(b"y", 0).wait(timeout=1e30) == b"y"


def test_await_and_wait_see_the_same_result(run: Runner) -> None:
    """Give awaiting tasks and wait the same result."""
    future = _testing.delayed(b"x", 0.05)

    async def one() -> bytes:
        return await future

    async def main() -> list[bytes]:
        return list(await asyncio.gather(one(), one()))

    assert run(main()) == [b"x", b"x"]
    assert future.wait() == b"x"


def test_wait_timeout_leaves_the_operation_running() -> None:
    """Raise TimeoutError from wait without stopping the operation."""
    future = _testing.delayed(b"x", 0.3)
    with pytest.raises(TimeoutError):
        future.wait(timeout=0.01)
    assert future.wait() == b"x"


def test_result_before_completion_raises() -> None:
    """Raise InvalidStateError when result is read before the operation finishes."""
    future = _testing.delayed(b"x", 5)
    with pytest.raises(asyncio.InvalidStateError):
        future.result()
    future.cancel()


def test_cancel_ends_the_operation() -> None:
    """Cancel a running operation once and report False afterwards."""
    future = _testing.delayed(b"x", 10)
    assert future.cancel()
    with pytest.raises(asyncio.CancelledError):
        future.wait()
    assert not future.cancel()


def test_cancelled_await_cancels_the_operation(run: Runner) -> None:
    """Cancel the operation when the awaiting task is cancelled."""
    future = _testing.delayed(b"x", 10)

    async def main() -> None:
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(future, 0.05)

    run(main())
    assert future.done()
    with pytest.raises(asyncio.CancelledError):
        future.result()


def test_completion_after_the_loop_closed(capfd: pytest.CaptureFixture[str]) -> None:
    """Finish quietly when the awaiting loop has already closed."""
    future = _testing.delayed(b"x", 0.5)

    async def waiter() -> bytes:
        return await future

    loop = asyncio.new_event_loop()
    loop.set_exception_handler(lambda loop, context: None)
    loop.create_task(waiter())
    loop.run_until_complete(asyncio.sleep(0.01))
    loop.close()
    assert asyncio.run(waiter()) == b"x"
    assert future.wait() == b"x"
    err = capfd.readouterr().err
    assert "panicked" not in err
    assert "Exception ignored" not in err


def test_panicking_operation_raises() -> None:
    """Raise SerialException from an operation whose task panicked."""
    future = _testing.panic_in_task()
    with pytest.raises(SerialException, match="panicked"):
        future.wait(timeout=5)
    assert future.done()


@pytest.mark.parametrize("delay", [-1, float("nan"), float("inf")])
def test_delayed_rejects_negative_delay(delay: float) -> None:
    """Reject negative and non-finite delays with ValueError."""
    with pytest.raises(ValueError):
        _testing.delayed(b"x", delay)


def test_cancel_always_reports_cancelled() -> None:
    """Report every cancel of a pending operation as successful."""
    futures = [_testing.delayed(b"x", 10) for _ in range(200)]
    for future in futures:
        assert future.cancel()
    for future in futures:
        with pytest.raises(asyncio.CancelledError):
            future.wait()


def test_callback_without_loop_runs_in_the_completing_thread() -> None:
    """Call a callback added outside a loop once, with the future, in another thread."""
    future = _testing.delayed(b"x", 0.05)
    calls: list[tuple[Any, int]] = []
    finished = threading.Event()

    def callback(f: Any) -> None:
        calls.append((f, threading.get_ident()))
        finished.set()

    future.add_done_callback(callback)
    assert future.wait() == b"x"
    assert finished.wait(5)
    assert len(calls) == 1
    assert calls[0][0] is future
    assert calls[0][1] != threading.get_ident()


def test_callback_runs_on_the_running_loop(run: Runner) -> None:
    """Run a callback added inside a running loop on that loop's thread."""
    future = _testing.delayed(b"x", 0.05)

    async def main() -> tuple[list[int], int]:
        threads: list[int] = []
        called = asyncio.get_running_loop().create_future()

        def callback(f: Any) -> None:
            threads.append(threading.get_ident())
            called.set_result(f)

        future.add_done_callback(callback)
        assert await asyncio.wait_for(called, 5) is future
        return threads, threading.get_ident()

    threads, loop_thread = run(main())
    assert threads == [loop_thread]


def test_callback_after_completion_without_loop_runs_at_once() -> None:
    """Call a callback added to a finished future before add_done_callback returns."""
    future = _testing.delayed(b"x", 0)
    future.wait()
    calls: list[Any] = []
    future.add_done_callback(calls.append)
    assert calls == [future]


def test_callback_after_completion_with_loop_runs_soon(run: Runner) -> None:
    """Schedule a callback added to a finished future on the running loop."""
    future = _testing.delayed(b"x", 0)

    async def main() -> tuple[list[Any], list[Any]]:
        await future
        calls: list[Any] = []
        future.add_done_callback(calls.append)
        before = list(calls)
        await asyncio.sleep(0)
        return before, calls

    before, after = run(main())
    assert before == []
    assert after == [future]


def test_remove_done_callback() -> None:
    """Remove every equal registration, report the count and skip removed callbacks."""
    future = _testing.delayed(b"x", 0.05)
    removed: list[Any] = []
    kept: list[Any] = []
    finished = threading.Event()
    future.add_done_callback(removed.append)
    future.add_done_callback(kept.append)
    future.add_done_callback(removed.append)
    future.add_done_callback(lambda f: finished.set())
    assert future.remove_done_callback(removed.append) == 2
    assert future.remove_done_callback(removed.append) == 0
    future.wait()
    assert finished.wait(5)
    assert removed == []
    assert kept == [future]


def test_callback_after_cancel_sees_cancelled() -> None:
    """Report the future as cancelled to a callback run by cancel."""
    future = _testing.delayed(b"x", 10)
    seen: list[bool] = []
    future.add_done_callback(lambda f: seen.append(f.cancelled()))
    assert future.cancel()
    assert seen == [True]


def test_exception() -> None:
    """Return the failure from exception and raise for cancelled or pending futures."""
    succeeded = _testing.delayed(b"x", 0)
    succeeded.wait()
    assert succeeded.exception() is None
    assert not succeeded.cancelled()

    failed = _testing.panic_in_task()
    with pytest.raises(SerialException):
        failed.wait(timeout=5)
    assert isinstance(failed.exception(), SerialException)
    assert not failed.cancelled()

    pending = _testing.delayed(b"x", 10)
    with pytest.raises(asyncio.InvalidStateError):
        pending.exception()
    assert not pending.cancelled()
    pending.cancel()
    with pytest.raises(asyncio.CancelledError):
        pending.exception()
    assert pending.cancelled()


def test_raising_callback_is_reported(monkeypatch: pytest.MonkeyPatch) -> None:
    """Report an exception from a callback and still call the next one."""
    future = _testing.delayed(b"x", 10)
    reports: list[Any] = []
    monkeypatch.setattr(sys, "unraisablehook", reports.append)
    calls: list[Any] = []

    def fail(f: Any) -> None:
        raise ValueError("callback failed")

    future.add_done_callback(fail)
    future.add_done_callback(calls.append)
    future.cancel()
    assert [type(r.exc_value) for r in reports] == [ValueError]
    assert calls == [future]


def test_callback_context() -> None:
    """Run callbacks in the given context or in a copy of the registering context."""
    var = contextvars.ContextVar("var", default="unset")
    given = contextvars.copy_context()
    given.run(var.set, "given")
    future = _testing.delayed(b"x", 10)
    seen: list[str] = []
    future.add_done_callback(lambda f: seen.append(var.get()), context=given)
    token = var.set("registered")
    future.add_done_callback(lambda f: seen.append(var.get()))
    var.reset(token)
    future.cancel()
    assert seen == ["given", "registered"]
