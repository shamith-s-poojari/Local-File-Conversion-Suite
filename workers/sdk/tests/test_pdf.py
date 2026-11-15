"""Shared PDF helpers (converter_sdk.pdf)."""

from __future__ import annotations

import pytest
from converter_sdk.pdf import RangeError, pages_in, parse_page_ranges


@pytest.mark.parametrize(
    ("spec", "expected"),
    [
        ("1-3, 5, 8-", [(1, 3), (5, 5), (8, 10)]),
        ("-2", [(1, 2)]),
        (" 4 ", [(4, 4)]),
        ("10-10,1", [(10, 10), (1, 1)]),
    ],
)
def test_parse_page_ranges(spec: str, expected) -> None:
    assert parse_page_ranges(spec, 10) == expected


@pytest.mark.parametrize(
    ("spec", "reason"),
    [
        ("", "empty"),
        ("0", "outOfBounds"),
        ("11", "outOfBounds"),
        ("3-2", "reversed"),
        ("1-2-3", "syntax"),
        ("x", "syntax"),
        ("-", "syntax"),
        ("1" * 10_001, "tooLong"),
    ],
)
def test_parse_page_ranges_rejects(spec: str, reason: str) -> None:
    with pytest.raises(RangeError) as e:
        parse_page_ranges(spec, 10)
    assert e.value.reason == reason


def test_pages_in_deduplicates_and_sorts() -> None:
    assert pages_in([(3, 4), (1, 3)]) == [1, 2, 3, 4]
