"""Failure categories: the likely cause of a failing Execution (ADR 0006).

Fixed, ordered regex rules, tried from most to least specific. The Failure
message is checked first; the Failure details only when the message matches
nothing. A heuristic: the UI calls the result "likely cause".
"""

import re
from collections import Counter
from typing import Literal

FailureCategory = Literal["network", "environment", "timing", "assertion", "other"]
CATEGORIES: tuple[FailureCategory, ...] = ("network", "environment", "timing", "assertion", "other")

_RULES: tuple[tuple[FailureCategory, re.Pattern[str]], ...] = tuple(
    (category, re.compile("|".join(patterns), re.IGNORECASE))
    for category, patterns in (
        (
            "network",
            (
                r"ECONNREFUSED",
                r"ECONNRESET",
                r"ENOTFOUND",
                r"EAI_AGAIN",
                r"ETIMEDOUT",
                r"EHOSTUNREACH",
                r"ENETUNREACH",
                r"socket hang up",
                r"net::ERR_",
                r"connection (?:refused|reset|aborted)",
                r"ConnectionError",
                r"ConnectionResetError",
                r"ConnectionRefusedError",
                r"Name or service not known",
                r"Temporary failure in name resolution",
                r"getaddrinfo",
                r"\b50[234] (?:Bad Gateway|Service Unavailable|Gateway Time-?out)",
            ),
        ),
        (
            "environment",
            (
                r"ENOSPC",
                r"No space left on device",
                r"MemoryError",
                r"out of memory",
                r"\bOOM\b",
                r"\bKilled\b",
                r"SIGKILL",
                r"signal 9\b",
                r"Permission denied",
                r"EACCES",
                r"Target (?:page, context or browser )?(?:has been )?closed",
                r"browser has been closed",
                r"ModuleNotFoundError",
                r"Cannot find module",
                r"No such file or directory",
                r"ENOENT",
                r"command not found",
            ),
        ),
        ("timing", (r"timeout", r"timed out", r"times out", r"deadline exceeded")),
        ("assertion", (r"AssertionError", r"\bassert", r"expect\(", r"\bexpected\b")),
    )
)


def _match(text: str) -> FailureCategory | None:
    for category, pattern in _RULES:
        if pattern.search(text):
            return category
    return None


def classify_failure(message: str, details: str) -> FailureCategory:
    """The Failure category of one failing Execution."""
    return _match(message) or _match(details) or "other"


def dominant_category(categories_newest_first: list[str | None]) -> FailureCategory | None:
    """Most common non-null category; the newest wins a tie; None when there is none."""
    present = [c for c in categories_newest_first if c is not None]
    if not present:
        return None
    counts = Counter(present)
    best = max(counts.values())
    return next(c for c in present if counts[c] == best)  # type: ignore[return-value]
