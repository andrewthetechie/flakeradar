"""Batch size for bulk statements and IN (...) lists.

asyncpg allows at most 32,767 bind parameters per statement, so every bulk
insert, update and IN list is split into chunks of CHUNK rows.
"""

from collections.abc import Iterator, Sequence
from typing import TypeVar

CHUNK = 1000

T = TypeVar("T")


def chunks(items: Sequence[T], size: int = CHUNK) -> Iterator[Sequence[T]]:
    for i in range(0, len(items), size):
        yield items[i : i + size]
