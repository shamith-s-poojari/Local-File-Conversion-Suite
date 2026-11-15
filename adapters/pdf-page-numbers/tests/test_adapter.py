"""Acceptance tests for pdf-page-numbers (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pymupdf
import pypdfium2 as pdfium
import pytest
from converter_conformance import corpus
from converter_conformance.harness import run_in_process
from PIL import ImageChops

ADAPTER = Path(__file__).resolve().parents[1]


def _words(path: Path) -> list[list[tuple[str, float, float]]]:
    """Each page's words with their positions as the reader sees them."""
    with pymupdf.open(path) as doc:
        return [
            [
                (w[4], (w[0] + w[2]) / 2 / p.rect.width, w[3] / p.rect.height)
                for w in p.get_text("words")
            ]
            for p in doc
        ]


def test_every_page_numbered_at_the_bottom_centre(tmp_path: Path) -> None:
    src = corpus.build("text-3", tmp_path / "corpus")
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"fileTitles": ["report"]})
    assert not r.errors
    assert r.names == ["report-numbered.pdf"]
    with pymupdf.open(r.outputs[0]) as doc:
        assert [p.get_text().count("Page 2 of 3") for p in doc] == [0, 1, 0]
    words = _words(r.outputs[0])
    of = next(w for w in words[0] if w[0] == "of")
    assert 0.45 < of[1] < 0.55 and of[2] > 0.9  # centred, at the bottom


def test_chosen_pages_count_from_the_first_number(tmp_path: Path) -> None:
    src = corpus.build("text-5", tmp_path / "corpus")
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [src],
        {"pages": "2-5", "startAt": 1, "style": "n", "position": "topRight", "text": "Draft"},
    )
    with pymupdf.open(r.outputs[0]) as doc:
        texts = [p.get_text() for p in doc]
    assert "Draft" not in texts[0]
    assert "Draft  ·  1" in texts[1].replace("\n", " ") or "Draft" in texts[1]
    words = _words(r.outputs[0])
    four = [w for w in words[4] if w[0] == "4"]
    assert four and four[-1][1] > 0.8 and four[-1][2] < 0.1  # top right


@pytest.mark.parametrize("rotate", [0, 90, 180, 270])
def test_numbers_are_upright_at_the_bottom_as_viewed(tmp_path: Path, rotate: int) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 1, rotate=rotate, outline=False)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"fontSize": 14})

    def render(path: Path):
        doc = pdfium.PdfDocument(path)
        image = doc[0].render(scale=1).to_pil().convert("L")
        doc.close()
        return image

    before, after = render(src), render(r.outputs[0])
    w, h = after.size
    added = ImageChops.difference(before, after).point(lambda v: 255 if v > 64 else 0)
    box = added.getbbox()  # only what the tool drew, as viewed
    assert box is not None, "nothing drawn"
    x0, y0, x1, y1 = box
    assert x1 - x0 > 3 * (y1 - y0), "text isn't horizontal"
    assert abs((x0 + x1) / 2 / w - 0.5) < 0.05, "not centred"
    assert y0 > h * 0.9, "not at the bottom"


def test_characters_helvetica_lacks_become_question_marks(tmp_path: Path) -> None:
    src = corpus.build("text-1", tmp_path / "corpus")
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"text": "Ωmega café"})
    assert "GLYPH_UNSUPPORTED" in r.warnings
    with pymupdf.open(r.outputs[0]) as doc:
        assert "?mega café" in doc[0].get_text()
