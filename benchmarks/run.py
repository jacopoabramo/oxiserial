"""Benchmarks comparing oxiserial with pyserial 3.5 that check every byte received."""

import struct


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
