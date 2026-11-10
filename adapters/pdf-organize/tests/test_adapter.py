"""Acceptance tests for pdf-organize (spec section 13.5)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
from converter_conformance import corpus
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]


def organize(tmp_path: Path, src: Path, pages: list[dict]):
    return run_in_process(ADAPTER, tmp_path / "job", [src], {"pages": pages})


def test_reorder_delete_duplicate(tmp_path) -> None:
    src = corpus.build("text-5", tmp_path / "corpus")
    source = corpus.page_texts(src)
    run = organize(tmp_path, src, [{"source": 3}, {"source": 1}, {"source": 3}])
    assert corpus.page_texts(run.outputs[0]) == [source[2], source[0], source[2]]


def test_rotation_is_relative_to_existing_rotation(tmp_path) -> None:
    src = corpus.build("rotated-90", tmp_path / "corpus")
    run = organize(tmp_path, src, [{"source": 1, "rotate": 90}, {"source": 2, "rotate": 270}])
    with pikepdf.open(run.outputs[0]) as pdf:
        assert [int(p.Rotate) % 360 for p in pdf.pages] == [180, 0]


def test_bookmarks_to_deleted_pages_are_removed(tmp_path) -> None:
    src = corpus.build("text-5", tmp_path / "corpus")
    run = organize(tmp_path, src, [{"source": 5}, {"source": 2}])
    with pikepdf.open(run.outputs[0]) as pdf, pdf.open_outline() as outline:
        index = {p.obj.objgen: i for i, p in enumerate(pdf.pages)}
        items = [(i.title, index[i.destination[0].objgen]) for i in outline.root]
    assert sorted(items) == [("Page 2", 1), ("Page 5", 0)]


def test_form_kept_when_pages_only_move(tmp_path) -> None:
    src = corpus.build("form", tmp_path / "corpus")
    run = organize(tmp_path, src, [{"source": 1, "rotate": 90}])
    assert "FORMS_FLATTENED" not in run.warnings
    with pikepdf.open(run.outputs[0]) as pdf:
        assert len(pdf.Root.AcroForm.Fields) == 1


def test_form_flattened_when_a_page_is_duplicated(tmp_path) -> None:
    src = corpus.build("form", tmp_path / "corpus")
    run = organize(tmp_path, src, [{"source": 1}, {"source": 1}])
    assert "FORMS_FLATTENED" in run.warnings


def test_empty_and_out_of_range_lists_are_rejected(tmp_path) -> None:
    src = corpus.build("text-3", tmp_path / "corpus")
    assert organize(tmp_path / "a", src, []).errors[0].detail.root["reason"] == "empty"
    run = organize(tmp_path / "b", src, [{"source": 4}])
    assert run.errors[0].detail.root["reason"] == "outOfBounds"
