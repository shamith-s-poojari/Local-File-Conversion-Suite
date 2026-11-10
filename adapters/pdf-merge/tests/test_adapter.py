"""Acceptance tests for pdf-merge (spec section 13.3)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
import pytest
from converter_conformance import corpus
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]


@pytest.fixture
def files(tmp_path: Path):
    return lambda *names: [corpus.build(n, tmp_path / "corpus") for n in names]


def merge(tmp_path: Path, inputs: list[Path], **params):
    return run_in_process(ADAPTER, tmp_path / "job", inputs, params)


def test_page_count_is_the_sum_and_content_is_unchanged(tmp_path, files) -> None:
    a, b = files("text-3", "text-5")
    run = merge(tmp_path, [a, b])
    assert run.names == ["merged.pdf"]
    merged = corpus.page_texts(run.outputs[0])
    assert merged == corpus.page_texts(a) + corpus.page_texts(b)


def test_mixed_sizes_and_rotations_are_preserved(tmp_path, files) -> None:
    run = merge(tmp_path, files("landscape", "rotated-90", "text-1"))
    with pikepdf.open(run.outputs[0]) as pdf:
        boxes = [tuple(float(v) for v in p.mediabox) for p in pdf.pages]
        rotations = [int(p.obj.get("/Rotate", 0)) for p in pdf.pages]
    assert boxes[:2] == [(0, 0, 792, 612)] * 2
    assert boxes[2:] == [(0, 0, 612, 792)] * 3
    assert rotations == [0, 0, 90, 90, 0]


def test_same_file_twice(tmp_path, files) -> None:
    (a,) = files("text-3")
    run = merge(tmp_path, [a, a])
    assert corpus.page_texts(run.outputs[0]) == corpus.page_texts(a) * 2


def test_per_file_bookmarks_point_at_the_right_pages(tmp_path, files) -> None:
    run = run_in_process(
        ADAPTER,
        tmp_path / "job",
        files("text-3", "text-5"),
        {"bookmarks": "perFile", "fileTitles": ["Intro", "Body"]},
    )
    with pikepdf.open(run.outputs[0]) as pdf, pdf.open_outline() as outline:
        top = list(outline.root)
        index = {p.obj.objgen: i for i, p in enumerate(pdf.pages)}
        target = lambda item: index[item.destination[0].objgen]  # noqa: E731
        assert [t.title for t in top] == ["Intro", "Body"]
        assert [target(t) for t in top] == [0, 3]
        assert [target(c) for c in top[1].children] == [3, 4, 5, 6, 7]
        assert [c.title for c in top[0].children] == ["Page 1", "Page 2", "Page 3"]


def test_bookmarks_none(tmp_path, files) -> None:
    run = merge(tmp_path, files("text-3", "text-3"), bookmarks="none")
    with pikepdf.open(run.outputs[0]) as pdf, pdf.open_outline() as outline:
        assert list(outline.root) == []


def test_colliding_form_fields_are_flattened_with_a_warning(tmp_path, files) -> None:
    run = merge(tmp_path, files("form", "form"))
    assert "FORMS_FLATTENED" in run.warnings
    with pikepdf.open(run.outputs[0]) as pdf:
        fields = pdf.Root.AcroForm.Fields
        assert len(fields) == 1  # the first file keeps its field
        widget = pdf.pages[0].Annots[0]
        assert widget.objgen == fields[0].objgen  # field and widget are one object
        assert "/Annots" not in pdf.pages[1] or len(pdf.pages[1].Annots) == 0


def test_tagged_input_warns_tags_dropped(tmp_path, files) -> None:
    run = merge(tmp_path, files("tagged", "text-1"))
    assert "TAGS_DROPPED" in run.warnings


def test_signed_input_warns_before_and_during(tmp_path, files) -> None:
    run = merge(tmp_path, files("signed", "text-1"))
    assert "SIGNATURE_INVALIDATED" in run.validation_warnings
    assert "SIGNATURE_INVALIDATED" in run.warnings


def test_metadata_from_first_file_by_default(tmp_path) -> None:
    a = corpus.text_pdf(tmp_path / "a.pdf", 1, title="First")
    b = corpus.text_pdf(tmp_path / "b.pdf", 1, title="Second")
    run = merge(tmp_path, [a, b])
    with pikepdf.open(run.outputs[0]) as pdf:
        assert str(pdf.docinfo["/Title"]) == "First"
    run = run_in_process(ADAPTER, tmp_path / "job2", [a, b], {"metadataFrom": -1})
    with pikepdf.open(run.outputs[0]) as pdf:
        assert "/Title" not in pdf.docinfo


def test_invalid_params_are_rejected(tmp_path, files) -> None:
    run = merge(tmp_path, files("text-1", "text-1"), metadataFrom=5)
    assert [e.param for e in run.errors] == ["metadataFrom"]
    assert run.outputs == []
