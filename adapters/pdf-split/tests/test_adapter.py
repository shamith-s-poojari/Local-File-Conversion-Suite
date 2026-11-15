"""Acceptance tests for pdf-split (spec section 13.4)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
import pytest
from converter_conformance import corpus
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]


@pytest.fixture
def ten(tmp_path: Path) -> Path:
    return corpus.build("text-10", tmp_path / "corpus")


def split(tmp_path: Path, src: Path, **params):
    return run_in_process(ADAPTER, tmp_path / "job", [src], params)


def pages_of(run) -> list[list[bytes]]:
    return [corpus.page_texts(p) for p in run.outputs]


def test_ranges_put_every_page_in_the_right_output(tmp_path, ten) -> None:
    source = corpus.page_texts(ten)
    run = split(tmp_path, ten, mode="ranges", ranges="1-3, 5, 8-")
    assert pages_of(run) == [source[0:3], source[4:5], source[7:10]]
    assert run.names == ["part-001.pdf", "part-002.pdf", "part-003.pdf"]


def test_every_n_and_each_page(tmp_path, ten) -> None:
    assert [len(p) for p in pages_of(split(tmp_path / "a", ten, mode="everyN", n=4))] == [4, 4, 2]
    each = split(tmp_path / "b", ten, mode="eachPage")
    assert len(each.outputs) == 10
    assert each.names[-1] == "part-010.pdf"


def test_output_holds_only_what_its_pages_need(tmp_path) -> None:
    heavy = corpus.build("heavy", tmp_path / "corpus")
    run = split(tmp_path, heavy, mode="ranges", ranges="1")
    # One page of 40, each ~2 MB of image: the part must not drag the rest along.
    assert run.outputs[0].stat().st_size < heavy.stat().st_size / 20


def test_max_size_groups_pages_under_the_limit(tmp_path) -> None:
    heavy = corpus.build("heavy", tmp_path / "corpus")
    run = split(tmp_path, heavy, mode="maxSize", maxSizeMb=7)
    counts = [len(corpus.page_texts(p)) for p in run.outputs]
    assert sum(counts) == 40
    assert len(counts) > 1
    for path in run.outputs[:-1]:
        assert path.stat().st_size <= 7 * 1024 * 1024 * 1.1  # best effort, small slack


def test_bookmarks_to_included_pages_are_kept(tmp_path, ten) -> None:
    run = split(tmp_path, ten, mode="ranges", ranges="4-6")
    with pikepdf.open(run.outputs[0]) as pdf, pdf.open_outline() as outline:
        assert [i.title for i in outline.root] == ["Page 4", "Page 5", "Page 6"]
    assert "OUTLINE_DROPPED" not in run.warnings


@pytest.mark.parametrize(
    ("ranges", "reason"),
    [
        ("0-2", "outOfBounds"),
        ("5-3", "reversed"),
        ("1-11", "outOfBounds"),
        ("a", "syntax"),
        (" ", "empty"),
    ],
)
def test_invalid_ranges_are_caught_in_validation(tmp_path, ten, ranges, reason) -> None:
    run = split(tmp_path, ten, mode="ranges", ranges=ranges)
    assert [(e.param, e.detail.root["reason"]) for e in run.errors] == [("ranges", reason)]
    assert run.outputs == []
