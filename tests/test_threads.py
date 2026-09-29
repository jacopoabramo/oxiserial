import threading
import time
from collections import Counter
from collections.abc import Callable, Generator

import pytest

from oxiserial import PortNotOpenError, Serial, serial_for_url

THREADS = 16
ROUNDS = 2000


@pytest.fixture(params=["loop", "mock"])
def ends(
    request: pytest.FixtureRequest, mock_pair: tuple[str, str]
) -> Generator[tuple[Serial, Serial], None, None]:
    """Provide a writing and a reading end: one loop:// port, or a mock pair."""
    if request.param == "loop":
        port = serial_for_url("loop://", timeout=0, write_timeout=5)
        yield port, port
        port.close()
    else:
        writer = Serial(mock_pair[0], timeout=0, write_timeout=5)
        reader = Serial(mock_pair[1], timeout=0, write_timeout=5)
        yield writer, reader
        writer.close()
        reader.close()


def mixed_operation(writer: Serial, reader: Serial, i: int) -> None:
    """Run the i-th non-I/O operation of the mix on the two ends."""
    match i % 4:
        case 0:
            writer.rts = bool(i & 8)
            writer.dtr = bool(i & 16)
        case 1:
            _ = reader.in_waiting, writer.out_waiting, reader.cts, reader.dsr
        case 2:
            writer.baudrate = 9600 if i & 8 else 115200
        case _:
            reader.apply_settings(reader.get_settings())


def run_threads(worker: Callable[[int], None]) -> list[BaseException]:
    """Run `worker(n)` on THREADS threads and return what they raised."""
    errors: list[BaseException] = []
    start = threading.Barrier(THREADS)

    def guarded(n: int) -> None:
        start.wait()
        try:
            worker(n)
        except BaseException as err:
            errors.append(err)

    threads = [threading.Thread(target=guarded, args=(n,)) for n in range(THREADS)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=30)
    assert not any(thread.is_alive() for thread in threads), "a thread is stuck"
    return errors


def test_mixed_operations_from_many_threads_lose_no_bytes(
    ends: tuple[Serial, Serial],
) -> None:
    """Deliver every byte once when many threads mix I/O and settings on one port."""
    writer, reader = ends
    written = [0] * THREADS
    received = [bytearray() for _ in range(THREADS)]

    def worker(n: int) -> None:
        for i in range(ROUNDS):
            if i % 3 == 0:
                written[n] += writer.write(bytes([n]) * 8) or 0
            elif i % 3 == 1:
                received[n] += reader.read(64)
            else:
                mixed_operation(writer, reader, i)

    assert run_threads(worker) == []
    rest = bytearray()
    while chunk := reader.read(4096):
        rest += chunk
    counts = Counter(b"".join(received) + rest)
    assert counts == Counter({n: written[n] for n in range(THREADS)})
    assert sum(written) == THREADS * 8 * len(range(0, ROUNDS, 3))


def test_close_during_mixed_operations_stops_every_thread(
    ends: tuple[Serial, Serial],
) -> None:
    """Raise only PortNotOpenError in threads using ports another thread closes."""
    writer, reader = ends

    def worker(n: int) -> None:
        i = 0
        while True:
            if i % 3 == 0:
                writer.write(bytes([n]) * 8)
            elif i % 3 == 1:
                reader.read(64)
            else:
                mixed_operation(writer, reader, i)
            i += 1

    def close_soon() -> None:
        time.sleep(0.2)
        writer.close()
        reader.close()

    closer = threading.Thread(target=close_soon)
    closer.start()
    errors = run_threads(worker)
    closer.join()
    assert len(errors) == THREADS
    assert all(isinstance(err, PortNotOpenError) for err in errors), errors
