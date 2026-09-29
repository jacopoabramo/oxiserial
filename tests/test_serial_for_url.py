import threading
import time

import pytest

import oxiserial
import oxiserial.aio
from conftest import Runner
from oxiserial import serial_for_url


def test_loop_returns_written_bytes_and_times_out_empty() -> None:
    """Read back what was written to loop:// and time out on an empty queue."""
    port = serial_for_url("loop://", baudrate=115200, timeout=0.01)
    assert port.is_open
    assert port.write(b"abc--") == 5
    assert port.read_until(expected=b"--") == b"abc--"
    start = time.perf_counter()
    assert port.read(1) == b""
    assert 0.005 < time.perf_counter() - start < 1
    port.close()


def test_loop_lines_buffers_and_close() -> None:
    """Read back the lines, drop queued bytes and close a loop:// port."""
    port = serial_for_url("loop://", timeout=0)
    port.dtr = False
    port.rts = True
    assert (port.dtr, port.rts, port.dsr, port.cts) == (False, True, False, True)
    port.rts = False
    assert (port.rts, port.cts) == (False, False)
    port.write(b"stale")
    port.reset_input_buffer()
    assert port.in_waiting == 0
    assert port.read(5) == b""
    port.close()
    assert not port.is_open


def test_unknown_scheme_raises() -> None:
    """Raise ValueError for a URL scheme other than loop://."""
    with pytest.raises(ValueError, match="protocol 'bogus' not known"):
        serial_for_url("bogus://x")


def test_do_not_open_returns_a_closed_port() -> None:
    """Return the port closed, with the URL as its port, when do_not_open is set."""
    port = serial_for_url("loop://", do_not_open=True)
    assert not port.is_open
    assert port.port == "loop://"


def test_device_name_opens_the_device(mock_pair: tuple[str, str]) -> None:
    """Open a device name as Serial does."""
    with (
        serial_for_url(mock_pair[0], 115200, timeout=1) as a,
        oxiserial.Serial(mock_pair[1], timeout=1) as b,
    ):
        assert a.baudrate == 115200
        a.write(b"hi")
        assert b.read(2) == b"hi"


def test_threads_sharing_a_loop_port_lose_no_bytes() -> None:
    """Keep every byte when threads take turns writing and reading one port."""
    port = serial_for_url("loop://", timeout=1)
    turn = threading.Lock()
    echoed: list[bytes] = []

    def worker(n: int) -> None:
        for i in range(50):
            message = f"<{n}:{i}>".encode()
            with turn:
                port.write(message)
                echoed.append(port.read_until(expected=b">"))

    threads = [threading.Thread(target=worker, args=(n,)) for n in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    port.close()
    assert sorted(echoed) == sorted(
        f"<{n}:{i}>".encode() for n in range(4) for i in range(50)
    )


def test_each_module_returns_its_serial_class() -> None:
    """Return oxiserial.Serial from oxiserial and the aio class from oxiserial.aio."""
    port = serial_for_url("loop://")
    aio_port = oxiserial.aio.serial_for_url("loop://")
    assert isinstance(port, oxiserial.Serial)
    assert isinstance(aio_port, oxiserial.aio.Serial)
    port.close()
    aio_port.close()


def test_aio_loop_round_trip_lines_and_close(run: Runner) -> None:
    """Pass the loop:// checks through oxiserial.aio.serial_for_url."""

    async def main() -> None:
        port = oxiserial.aio.serial_for_url("loop://", baudrate=115200, timeout=0.01)
        assert port.is_open
        assert await port.write(b"abc--") == 5
        assert await port.read_until(expected=b"--") == b"abc--"
        start = time.perf_counter()
        assert await port.read(1) == b""
        assert 0.005 < time.perf_counter() - start < 1
        port.dtr = False
        port.rts = False
        assert (port.dtr, port.rts, port.dsr, port.cts) == (False, False, False, False)
        await port.write(b"stale")
        port.reset_input_buffer()
        assert await port.read(5) == b""
        port.close()
        assert not port.is_open

    run(main())


def test_aio_url_errors_and_do_not_open() -> None:
    """Raise for an unknown scheme and leave the port closed with do_not_open."""
    with pytest.raises(ValueError, match="protocol 'bogus' not known"):
        oxiserial.aio.serial_for_url("bogus://x")
    port = oxiserial.aio.serial_for_url("loop://", do_not_open=True)
    assert not port.is_open
    assert port.port == "loop://"


def test_aio_device_name_opens_the_device(
    mock_pair: tuple[str, str], run: Runner
) -> None:
    """Open a device name through oxiserial.aio.serial_for_url."""

    async def main() -> bytes:
        async with (
            oxiserial.aio.serial_for_url(mock_pair[0], timeout=1) as a,
            oxiserial.aio.Serial(mock_pair[1], timeout=1) as b,
        ):
            await a.write(b"hi")
            return await b.read(2)

    assert run(main()) == b"hi"
