"""Acceptance tests for pdf-to-image (ADR 0003)."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml
from converter_conformance import corpus
from converter_conformance.harness import load_adapter_class, run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]


def run(tmp_path: Path, src: Path, to: str, secrets=None, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["report"], **params}, secrets
    )


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == load_adapter_class(ADAPTER).EDGES


@pytest.mark.parametrize("to", ["png", "jpg", "webp", "gif", "bmp"])
def test_one_image_per_page(tmp_path: Path, to: str) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 3)
    r = run(tmp_path, src, to, dpi=72)
    assert r.names == [f"report-page-{n}.{to}" for n in (1, 2, 3)]
    assert r.sources == [0, 0, 0]
    with Image.open(r.outputs[0]) as img:
        assert img.size == (612, 792)  # Letter at 72 dpi


def test_page_selection_and_single_page_name(tmp_path: Path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 5)
    r = run(tmp_path, src, "png", pages="2")
    assert r.names == ["report.png"]
    r = run(tmp_path / "b", src, "png", pages="4-")
    assert r.names == ["report-page-4.png", "report-page-5.png"]


def test_dpi_scales_the_image(tmp_path: Path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 1)
    r = run(tmp_path, src, "png", dpi=300)
    with Image.open(r.outputs[0]) as img:
        assert abs(img.width - 2550) <= 1 and abs(img.height - 3300) <= 1


def test_multipage_tiff(tmp_path: Path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 3)
    r = run(tmp_path, src, "tiff", dpi=72)
    assert r.names == ["report.tiff"]
    with Image.open(r.outputs[0]) as img:
        assert img.n_frames == 3
    r = run(tmp_path / "b", src, "tiff", dpi=72, multipage=False)
    assert len(r.outputs) == 3


def test_svg_export_keeps_vectors(tmp_path: Path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 2)
    r = run(tmp_path, src, "svg")
    assert len(r.outputs) == 2
    assert r.outputs[0].read_text("utf-8").lstrip().startswith(("<?xml", "<svg"))


def test_password_protected_input(tmp_path: Path) -> None:
    src = corpus.encrypted_pdf(tmp_path / "src.pdf")
    locked = run(tmp_path / "a", src, "png")
    assert locked.errors and locked.errors[0].code == "E_PASSWORD_REQUIRED"
    opened = run(tmp_path / "b", src, "png", secrets={"password": corpus.SENTINEL_PASSWORD})
    assert not opened.errors and opened.outputs


def test_bad_page_range_is_a_param_error(tmp_path: Path) -> None:
    src = corpus.text_pdf(tmp_path / "src.pdf", 2)
    r = run(tmp_path, src, "png", pages="9")
    assert r.errors and r.errors[0].param == "pages"
