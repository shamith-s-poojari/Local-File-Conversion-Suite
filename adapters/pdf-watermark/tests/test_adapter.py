"""Acceptance tests for pdf-watermark (spec section 13.7).

Placement is checked by rendering with pdfium: the watermark is pure red,
the corpus is black, so red pixels show exactly where the stamp landed.
"""

from __future__ import annotations

from pathlib import Path

import pikepdf
import pypdfium2 as pdfium
import pytest
from converter_conformance import corpus
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]
RED = {"text": "MARK", "color": "#FF0000", "opacity": 1.0, "rotation": 0, "fontSize": 40}


def stamp(tmp_path: Path, src: Path, **params):
    return run_in_process(ADAPTER, tmp_path / "job", [src], {**RED, **params})


def red_centroid(path: Path, page: int = 0) -> tuple[float, float] | None:
    """Centre of red pixels as fractions of the rendered width and height
    (origin top-left, as the viewer sees it)."""
    doc = pdfium.PdfDocument(path)
    image = doc[page].render(scale=0.5).to_pil().convert("RGB")
    doc.close()
    w, h = image.size
    xs = ys = n = 0
    for i, (r, g, b) in enumerate(image.getdata()):
        if r > 200 and g < 90 and b < 90:
            xs += i % w
            ys += i // w
            n += 1
    return (xs / n / w, ys / n / h) if n else None


CORNERS = {
    "topLeft": lambda x, y: x < 0.5 and y < 0.5,
    "topRight": lambda x, y: x > 0.5 and y < 0.5,
    "bottomLeft": lambda x, y: x < 0.5 and y > 0.5,
    "bottomRight": lambda x, y: x > 0.5 and y > 0.5,
}


@pytest.mark.parametrize("rotate", [0, 90, 180, 270])
@pytest.mark.parametrize("position", list(CORNERS))
def test_placement_follows_what_the_viewer_sees(tmp_path, rotate, position) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 1, rotate=rotate, outline=False)
    run = stamp(tmp_path, src, position=position)
    centre = red_centroid(run.outputs[0])
    assert centre is not None
    assert CORNERS[position](*centre), centre


def test_center_and_nonzero_cropbox_origin(tmp_path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 1, crop_origin=(100, 150), outline=False)
    centre = red_centroid(stamp(tmp_path, src, position="center").outputs[0])
    assert centre is not None
    assert abs(centre[0] - 0.5) < 0.05 and abs(centre[1] - 0.5) < 0.05


def test_pages_outside_the_selection_are_untouched(tmp_path) -> None:
    src = corpus.build("text-5", tmp_path / "corpus")
    run = stamp(tmp_path, src, pages="2, 4")
    before, after = corpus.page_texts(src), corpus.page_texts(run.outputs[0])
    assert [after[i] == before[i] for i in range(5)] == [True, False, True, False, True]
    assert red_centroid(run.outputs[0], 0) is None
    assert red_centroid(run.outputs[0], 1) is not None


def test_under_layer_draws_first(tmp_path) -> None:
    src = corpus.build("text-1", tmp_path / "corpus")
    run = stamp(tmp_path, src, layer="under")
    with pikepdf.open(run.outputs[0]) as pdf:
        page = pdf.pages[0]
        page.contents_coalesce()
        assert page.Contents.read_bytes().startswith(b"q ")


def test_original_text_stays_extractable(tmp_path) -> None:
    src = corpus.build("text-3", tmp_path / "corpus")
    out = stamp(tmp_path, src, position="tile", opacity=0.3).outputs[0]
    doc = pdfium.PdfDocument(out)
    text = doc[1].get_textpage().get_text_bounded()
    doc.close()
    assert "Page 2" in text


def test_one_shared_stamp_keeps_large_jobs_small(tmp_path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 300, outline=False)
    out = stamp(tmp_path, src).outputs[0]
    with pikepdf.open(out) as pdf:
        stamps = {
            v.objgen
            for p in pdf.pages
            for k, v in p.Resources.XObject.items()
            if k.startswith("/LfcsWm")
        }
    assert len(stamps) == 1
    assert out.stat().st_size < src.stat().st_size * 1.5


def test_unsupported_characters_warn(tmp_path) -> None:
    src = corpus.build("text-1", tmp_path / "corpus")
    run = stamp(tmp_path, src, text="Draft ✓ 日本")
    assert "GLYPH_UNSUPPORTED" in run.warnings


@pytest.mark.parametrize(
    ("params", "param"),
    [
        ({"text": ""}, "text"),
        ({"color": "red"}, "color"),
        ({"pages": "9"}, "pages"),
        ({"type": "image"}, "type"),
    ],
)
def test_invalid_params(tmp_path, params, param) -> None:
    src = corpus.build("text-3", tmp_path / "corpus")
    run = stamp(tmp_path, src, **params)
    assert [e.param for e in run.errors] == [param]
