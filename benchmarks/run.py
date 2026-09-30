"""Benchmarks comparing oxiserial with pyserial 3.5 that check every byte received."""

import argparse
import asyncio
import itertools
import os
import statistics
import struct
import sys
import threading
import time
from collections.abc import Awaitable, Callable, Coroutine
from dataclasses import dataclass
from typing import Any, Protocol, TypeVar, runtime_checkable

import rsloop
import serial

import oxiserial
import oxiserial.aio
from oxiserial.aio import Serial as AioSerial

WARMUP = 20
MAX_SAMPLES = 10_000
STREAM_SIZE = 1 << 20
# pyserial's loop:// rejects a write whose time at this rate exceeds write_timeout;
# 1 MiB takes 11.4 s at 921600 baud.
BAUDRATE = 921_600
READ_TIMEOUT = 1.0
WRITE_TIMEOUT = 30.0


class IntegrityError(Exception):
    """Received data differs from the data sent."""


def pattern(size: int) -> bytes:
    """Return `size` bytes of consecutive 4-byte big-endian counters."""
    words = (size + 3) // 4
    return struct.pack(f">{words}I", *range(words))[:size]


def check(bench: str, expected: bytes, got: bytes) -> None:
    """Raise if `got` differs from `expected`.

    Raises
    ------
    IntegrityError
        If the two differ, naming the offset of the first wrong byte.
    """
    if got == expected:
        return
    offset = next(
        (i for i, (a, b) in enumerate(zip(expected, got, strict=False)) if a != b),
        min(len(expected), len(got)),
    )
    raise IntegrityError(
        f"{bench}: first wrong byte at offset {offset}; "
        f"sent {len(expected)} bytes, received {len(got)}"
    )


PAYLOAD = pattern(STREAM_SIZE)


class SyncPort(Protocol):
    """The blocking port API shared by pyserial, oxiserial and `WaitPort`."""

    @property
    def in_waiting(self) -> int: ...
    @property
    def timeout(self) -> float | None: ...
    @timeout.setter
    def timeout(self, value: float | None) -> None: ...
    def write(self, data: bytes, /) -> int | None: ...
    def read(self, size: int = 1, /) -> bytes: ...
    def readline(self, size: int = -1, /) -> bytes: ...
    def read_until(
        self, expected: bytes = b"\n", size: int | None = None, /
    ) -> bytes: ...
    def close(self) -> None: ...


@runtime_checkable
class ReadIntoPort(SyncPort, Protocol):
    """A `SyncPort` that also has `readinto`."""

    def readinto(self, buffer: bytearray, /) -> int | None: ...


class WaitPort:
    """An `oxiserial.aio.Serial` whose I/O methods wait for their future."""

    def __init__(self, port: AioSerial) -> None:
        self.port = port

    @property
    def in_waiting(self) -> int:
        return self.port.in_waiting

    @property
    def timeout(self) -> float | None:
        return self.port.timeout

    @timeout.setter
    def timeout(self, value: float | None) -> None:
        self.port.timeout = value

    def write(self, data: bytes, /) -> int | None:
        return self.port.write(data).wait()

    def read(self, size: int = 1, /) -> bytes:
        return self.port.read(size).wait()

    def readline(self, size: int = -1, /) -> bytes:
        return self.port.readline(size).wait()

    def read_until(self, expected: bytes = b"\n", size: int | None = None, /) -> bytes:
        return self.port.read_until(expected, size).wait()

    def close(self) -> None:
        self.port.close()


class Closable(Protocol):
    def close(self) -> None: ...


P = TypeVar("P", bound=Closable)
Samples = list[float] | None
SyncBench = Callable[[SyncPort, SyncPort, float], Samples]
AsyncBench = Callable[[AioSerial, AioSerial, float], Coroutine[Any, Any, Samples]]


def sample(
    step: Callable[[], None],
    budget: float,
    setup: Callable[[], None] | None = None,
    warmup: int = WARMUP,
) -> list[float]:
    """Time `step` until `budget` seconds pass, returning seconds per call.

    `setup` runs before each call, outside the timed part. At least one call
    is timed, however long it takes.
    """
    for _ in range(warmup):
        if setup is not None:
            setup()
        step()
    times: list[float] = []
    end = time.perf_counter() + budget
    while not times or (len(times) < MAX_SAMPLES and time.perf_counter() < end):
        if setup is not None:
            setup()
        start = time.perf_counter()
        step()
        times.append(time.perf_counter() - start)
    return times


async def sample_async(
    step: Callable[[], Awaitable[None]],
    budget: float,
    setup: Callable[[], Awaitable[None]] | None = None,
    warmup: int = WARMUP,
) -> list[float]:
    """Time the coroutine `step` as [`sample`][benchmarks.run.sample] does."""
    for _ in range(warmup):
        if setup is not None:
            await setup()
        await step()
    times: list[float] = []
    end = time.perf_counter() + budget
    while not times or (len(times) < MAX_SAMPLES and time.perf_counter() < end):
        if setup is not None:
            await setup()
        start = time.perf_counter()
        await step()
        times.append(time.perf_counter() - start)
    return times


def line(i: int) -> bytes:
    """Return a 64-byte line ending in a newline, unique per `i`."""
    return f"{i:063x}\n".encode()


def dashed(i: int) -> bytes:
    """Return a 64-byte message ending in `--`, unique per `i`."""
    return f"{i:062x}--".encode()


def l1(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    counter = itertools.count()

    def step() -> None:
        data = struct.pack(">Q", next(counter))
        w.write(data)
        check("L1", data, r.read(8))

    return sample(step, budget)


async def l1_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    counter = itertools.count()

    async def step() -> None:
        data = struct.pack(">Q", next(counter))
        await w.write(data)
        check("L1", data, await r.read(8))

    return await sample_async(step, budget)


def l2_readline(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    counter = itertools.count()

    def step() -> None:
        data = line(next(counter))
        w.write(data)
        check("L2 readline", data, r.readline())

    return sample(step, budget)


async def l2_readline_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    counter = itertools.count()

    async def step() -> None:
        data = line(next(counter))
        await w.write(data)
        check("L2 readline", data, await r.readline())

    return await sample_async(step, budget)


def l2_read_until(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    counter = itertools.count()

    def step() -> None:
        data = dashed(next(counter))
        w.write(data)
        check("L2 read_until", data, r.read_until(b"--"))

    return sample(step, budget)


async def l2_read_until_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    counter = itertools.count()

    async def step() -> None:
        data = dashed(next(counter))
        await w.write(data)
        check("L2 read_until", data, await r.read_until(b"--"))

    return await sample_async(step, budget)


def l3(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    counter = itertools.count()
    queued: list[bytes] = []

    def setup() -> None:
        data = struct.pack(">Q", next(counter))
        queued.append(data)
        w.write(data)
        deadline = time.perf_counter() + READ_TIMEOUT
        while r.in_waiting < len(data):
            if time.perf_counter() > deadline:
                raise IntegrityError("L3: written bytes did not arrive")

    def step() -> None:
        check("L3", queued.pop(), r.read(r.in_waiting))

    return sample(step, budget, setup)


async def l3_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    counter = itertools.count()
    queued: list[bytes] = []

    async def setup() -> None:
        data = struct.pack(">Q", next(counter))
        queued.append(data)
        await w.write(data)
        deadline = time.perf_counter() + READ_TIMEOUT
        while r.in_waiting < len(data):
            if time.perf_counter() > deadline:
                raise IntegrityError("L3: written bytes did not arrive")
            await asyncio.sleep(0)

    async def step() -> None:
        check("L3", queued.pop(), await r.read(r.in_waiting))

    return await sample_async(step, budget, setup)


def l4(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    r.timeout = 0

    def step() -> None:
        check("L4", b"", r.read(1))

    try:
        return sample(step, budget)
    finally:
        r.timeout = READ_TIMEOUT


async def l4_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    r.timeout = 0

    async def step() -> None:
        check("L4", b"", await r.read(1))

    try:
        return await sample_async(step, budget)
    finally:
        r.timeout = READ_TIMEOUT


def l5(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    values = itertools.cycle([0.5, READ_TIMEOUT])

    def step() -> None:
        r.timeout = next(values)

    try:
        return sample(step, budget)
    finally:
        r.timeout = READ_TIMEOUT


def stream(
    bench: str, w: SyncPort, r: SyncPort, budget: float, receive: Callable[[], bytes]
) -> list[float]:
    """Time sending `PAYLOAD` from a writer thread while `receive` collects it."""

    def step() -> None:
        errors: list[Exception] = []

        def write() -> None:
            try:
                w.write(PAYLOAD)
            except Exception as err:
                errors.append(err)

        writer = threading.Thread(target=write)
        writer.start()
        got = receive()
        writer.join()
        if errors:
            raise errors[0]
        check(bench, PAYLOAD, got)

    return sample(step, budget, warmup=0)


def s1(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    def receive() -> bytes:
        out = bytearray()
        while len(out) < STREAM_SIZE:
            chunk = r.read(4096)
            if not chunk:
                break
            out += chunk
        return bytes(out)

    return stream("S1", w, r, budget, receive)


def s2(w: SyncPort, r: SyncPort, budget: float) -> list[float] | None:
    if not isinstance(r, ReadIntoPort):
        return None
    reader = r
    buffer = bytearray(4096)
    view = memoryview(buffer)

    def receive() -> bytes:
        out = bytearray()
        while len(out) < STREAM_SIZE:
            n = reader.readinto(buffer)
            if not n:
                break
            out += view[:n]
        return bytes(out)

    return stream("S2", w, r, budget, receive)


def s3(w: SyncPort, r: SyncPort, budget: float) -> list[float]:
    r.timeout = 10 * READ_TIMEOUT
    try:
        return stream("S3", w, r, budget, lambda: r.read(STREAM_SIZE))
    finally:
        r.timeout = READ_TIMEOUT


async def stream_async(
    bench: str,
    w: AioSerial,
    budget: float,
    receive: Callable[[], Awaitable[bytes]],
) -> list[float]:
    """Time sending `PAYLOAD` while `receive` collects it on the same event loop."""

    async def step() -> None:
        _, got = await asyncio.gather(w.write(PAYLOAD), receive())
        check(bench, PAYLOAD, got)

    return await sample_async(step, budget, warmup=0)


async def s1_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    async def receive() -> bytes:
        out = bytearray()
        while len(out) < STREAM_SIZE:
            chunk = await r.read(4096)
            if not chunk:
                break
            out += chunk
        return bytes(out)

    return await stream_async("S1", w, budget, receive)


async def s3_async(w: AioSerial, r: AioSerial, budget: float) -> list[float]:
    r.timeout = 10 * READ_TIMEOUT

    async def receive() -> bytes:
        return await r.read(STREAM_SIZE)

    try:
        return await stream_async("S3", w, budget, receive)
    finally:
        r.timeout = READ_TIMEOUT


@dataclass(frozen=True)
class Bench:
    """One row of the output table."""

    id: str
    label: str
    streaming: bool
    sync: SyncBench
    run_async: AsyncBench | None


BENCHES = (
    Bench("L1", "write(8), read(8)", False, l1, l1_async),
    Bench("L2", "64-byte line, readline()", False, l2_readline, l2_readline_async),
    Bench(
        "L2",
        "64-byte message, read_until(b'--')",
        False,
        l2_read_until,
        l2_read_until_async,
    ),
    Bench("L3", "in_waiting, read(in_waiting)", False, l3, l3_async),
    Bench("L4", "read(1) on an empty port, timeout=0", False, l4, l4_async),
    Bench("L5", "timeout = x", False, l5, None),
    Bench("S1", "1 MiB, read(4096)", True, s1, s1_async),
    Bench("S2", "1 MiB, readinto(4096 bytes)", True, s2, None),
    Bench("S3", "1 MiB, one read(1 MiB)", True, s3, s3_async),
)
BENCH_IDS = tuple(dict.fromkeys(bench.id for bench in BENCHES))


def open_pyserial(name: str) -> SyncPort:
    port: SyncPort = serial.serial_for_url(
        name, baudrate=BAUDRATE, timeout=READ_TIMEOUT, write_timeout=WRITE_TIMEOUT
    )
    return port


def open_oxiserial(name: str) -> SyncPort:
    return oxiserial.serial_for_url(
        name, baudrate=BAUDRATE, timeout=READ_TIMEOUT, write_timeout=WRITE_TIMEOUT
    )


def open_aio(name: str) -> AioSerial:
    return oxiserial.aio.serial_for_url(
        name, baudrate=BAUDRATE, timeout=READ_TIMEOUT, write_timeout=WRITE_TIMEOUT
    )


SYNC_LIBRARIES: dict[str, Callable[[str], SyncPort]] = {
    "pyserial": open_pyserial,
    "oxiserial": open_oxiserial,
    "aio .wait()": lambda name: WaitPort(open_aio(name)),
}
ASYNC_LIBRARIES: dict[str, Callable[[Coroutine[Any, Any, Samples]], Samples]] = {
    "aio asyncio": asyncio.run,
    "aio rsloop": rsloop.run,
}


@dataclass(frozen=True)
class PortSpec:
    """A port to benchmark: `a` writes and `b` reads; the same name means one port."""

    label: str
    a: str
    b: str
    pyserial: bool


def open_ends(spec: PortSpec, opener: Callable[[str], P]) -> tuple[P, P]:
    """Open the writing and reading ends, sharing one port when the names match."""
    writer = opener(spec.a)
    if spec.a == spec.b:
        return writer, writer
    try:
        return writer, opener(spec.b)
    except BaseException:
        writer.close()
        raise


def run_case(
    spec: PortSpec, bench: Bench, library: str, budget: float
) -> list[float] | None:
    """Run one benchmark on one port for one library; `None` means it does not apply."""
    if library in SYNC_LIBRARIES:
        w, r = open_ends(spec, SYNC_LIBRARIES[library])
        try:
            return bench.sync(w, r, budget)
        finally:
            w.close()
            r.close()
    if bench.run_async is None:
        return None
    aw, ar = open_ends(spec, open_aio)
    try:
        return ASYNC_LIBRARIES[library](bench.run_async(aw, ar, budget))
    finally:
        aw.close()
        ar.close()


def cell(times: list[float], bench: Bench, baseline: list[float] | None) -> str:
    """Format one result, with its time relative to pyserial when there is one."""
    median = statistics.median(times)
    if bench.streaming:
        text = f"{STREAM_SIZE / median / 1e6:.1f} MB/s"
    else:
        p95 = statistics.quantiles(times, n=20)[-1] if len(times) > 1 else times[0]
        text = f"{median * 1e6:.1f} us (p95 {p95 * 1e6:.1f})"
    if baseline:
        text += f", {median / statistics.median(baseline):.2f}x"
    return text


def run_port(spec: PortSpec, ids: set[str], budget: float) -> int:
    """Print the table for one port and return the number of failed cases."""
    libraries = [*SYNC_LIBRARIES, *ASYNC_LIBRARIES]
    if not spec.pyserial:
        libraries.remove("pyserial")
    failures = 0
    rows: list[str] = []
    for bench in BENCHES:
        if bench.id not in ids:
            continue
        results: dict[str, list[float] | None] = {}
        notes: dict[str, str] = {}
        for library in libraries:
            try:
                results[library] = run_case(spec, bench, library, budget)
            except Exception as err:
                failures += 1
                results[library] = None
                notes[library] = f"FAILED: {type(err).__name__}: {err}"
        baseline = results.get("pyserial")
        cells = [
            notes.get(library)
            or (cell(times, bench, baseline) if (times := results[library]) else "-")
            for library in libraries
        ]
        rows.append(f"| {bench.id} | {bench.label} | " + " | ".join(cells) + " |")
    print(f"\n### {spec.label}\n")
    print("| id | benchmark | " + " | ".join(libraries) + " |")
    print("|---|---|" + "---|" * len(libraries))
    print("\n".join(rows))
    return failures


def parse_ids(text: str) -> set[str]:
    ids = {item.strip() for item in text.split(",") if item.strip()}
    unknown = ids - set(BENCH_IDS)
    if unknown:
        raise argparse.ArgumentTypeError(
            f"unknown id {', '.join(sorted(unknown))}; "
            f"valid ids: {', '.join(BENCH_IDS)}"
        )
    return ids


def main(argv: list[str] | None = None) -> int:
    """Run the benchmarks and print Markdown tables; return 1 if any case failed."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port-a", default=os.environ.get("OXISERIAL_PORT_A"))
    parser.add_argument("--port-b", default=os.environ.get("OXISERIAL_PORT_B"))
    parser.add_argument("--only", type=parse_ids, default=set(BENCH_IDS))
    parser.add_argument("--budget", type=float, default=1.0)
    args = parser.parse_args(argv)

    print("## oxiserial benchmarks\n")
    print(
        "Times are medians per operation; the ratio is time relative to pyserial "
        "(above 1 is slower). The numbers assume a release build "
        "(`maturin develop --release`).\n"
    )
    ports = [PortSpec("loop://", "loop://", "loop://", pyserial=True)]
    testing = getattr(oxiserial, "_testing", None)
    if testing is not None:
        a, b = testing.mock_pair()
        ports.append(PortSpec("mock pair", a, b, pyserial=False))
    else:
        print("Mock pair skipped: this build has no `test-backend` feature.\n")
    if args.port_a and args.port_b:
        ports.append(
            PortSpec(
                f"{args.port_a} -> {args.port_b}",
                args.port_a,
                args.port_b,
                pyserial=True,
            )
        )
    failures = sum(run_port(spec, args.only, args.budget) for spec in ports)
    if failures:
        print(f"\n{failures} case(s) failed.")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
