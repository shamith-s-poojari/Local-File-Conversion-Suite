"""Acceptance tests for images-to-pdf (ADR 0003)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
from converter_conformance import files
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]


def test_pages_follow_input_order_and_sizes(tmp_path: Path) -> None:
    srcs = [files.image_file(tmp_path / f"in.{f}", f) for f in ("png", "jpg", "heic", "gif")]
    r = run_in_process(ADAPTER, tmp_path / "job", srcs, {})
    assert not r.errors
    assert r.names == ["images.pdf"]
    with pikepdf.open(r.outputs[0]) as pdf:
        assert len(pdf.pages) == 4
        boxes = [tuple(round(float(x)) for x in p.mediabox) for p in pdf.pages]
        assert boxes[0] == (0, 0, 320, 200)
        assert boxes[1] == (0, 0, 200, 320)  # EXIF-rotated JPEG is upright
        assert pdf.check_pdf_syntax() == []


def test_a4_pages_with_margins(tmp_path: Path) -> None:
    srcs = [files.image_file(tmp_path / "in.png", "png")]
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        srcs,
        {"pageSize": "a4", "margin": "small", "fileTitles": ["scan"]},
    )
    assert r.names == ["scan.pdf"]
    with pikepdf.open(r.outputs[0]) as pdf:
        w, h = (float(x) for x in list(pdf.pages[0].mediabox)[2:])
        assert round(w) == 842 and round(h) == 595  # landscape image, landscape A4
