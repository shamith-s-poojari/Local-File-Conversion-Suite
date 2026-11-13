"""Acceptance tests for image-convert (ADR 0003)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
import pytest
import yaml
from converter_conformance import files
from converter_conformance.harness import load_adapter_class, run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)
PIL_NAMES = {
    "png": "PNG", "jpg": "JPEG", "webp": "WEBP", "gif": "GIF", "bmp": "BMP",
    "tiff": "TIFF", "avif": "AVIF", "ico": "ICO",
}  # fmt: skip


def convert(tmp_path: Path, src: Path, to: str, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["holiday"], **params}
    )


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize(("src", "to"), sorted(CLS.EDGES))
def test_every_edge_produces_a_readable_file(tmp_path: Path, src: str, to: str) -> None:
    source = files.image_file(tmp_path / f"in.{src}", src)
    run = convert(tmp_path, source, to)
    assert not run.errors, run.errors
    assert run.names == [f"holiday.{to}"]
    out = run.outputs[0]
    if to == "pdf":
        with pikepdf.open(out) as pdf:
            assert len(pdf.pages) == 1
            assert pdf.check_pdf_syntax() == []
        return
    with Image.open(out) as img:
        assert img.format == PIL_NAMES[to]
        assert img.width > 0 and img.height > 0


def test_exif_orientation_is_applied(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.jpg", "jpg")
    with Image.open(src) as raw:
        assert raw.width > raw.height  # stored sideways
    run = convert(tmp_path, src, "png")
    with Image.open(run.outputs[0]) as img:
        assert img.height > img.width  # upright


def test_transparency_to_jpg_is_filled_and_warned(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    run = convert(tmp_path, src, "jpg", background="#00FF00")
    assert "TRANSPARENCY_FLATTENED" in run.warnings
    with Image.open(run.outputs[0]) as img:
        assert img.convert("RGB").getpixel((img.width - 2, img.height - 2)) == pytest.approx(
            (0, 255, 0), abs=12
        )


def test_animation_survives_to_webp_but_not_png(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.gif", "gif")
    webp = convert(tmp_path / "a", src, "webp")
    with Image.open(webp.outputs[0]) as img:
        assert getattr(img, "n_frames", 1) == 4
    png = convert(tmp_path / "b", src, "png")
    assert "ANIMATION_FLATTENED" in png.warnings


def test_png_to_webp_is_lossless(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    run = convert(tmp_path, src, "webp")
    with Image.open(src) as a, Image.open(run.outputs[0]) as b:
        assert a.convert("RGBA").tobytes() == b.convert("RGBA").tobytes()


def test_resize_only_shrinks(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    run = convert(tmp_path, src, "tiff", maxSide=100)
    assert "IMAGE_DOWNSCALED" in run.warnings
    with Image.open(run.outputs[0]) as img:
        assert max(img.size) == 100
    run = convert(tmp_path / "big", src, "tiff", maxSide=3840)
    with Image.open(run.outputs[0]) as img:
        assert img.size == (320, 200)


def test_strip_metadata_removes_exif(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.jpg", "jpg")
    kept = convert(tmp_path / "k", src, "webp")
    with Image.open(kept.outputs[0]) as img:
        assert img.getexif().get(0x010F) == "Fixture Camera"
        assert img.getexif().get(0x0112) is None  # orientation already applied
    stripped = convert(tmp_path / "s", src, "webp", stripMetadata=True)
    with Image.open(stripped.outputs[0]) as img:
        assert not img.getexif()


def test_jpeg_goes_into_pdf_without_reencoding(tmp_path: Path) -> None:
    src = tmp_path / "flat.jpg"
    files._picture().save(src, "JPEG", quality=77)
    run = convert(tmp_path, src, "pdf")
    with pikepdf.open(run.outputs[0]) as pdf:
        images = list(pdf.pages[0].images.values())
        assert images[0].read_raw_bytes() == src.read_bytes()


def test_svg_to_pdf_stays_vector(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.svg", "svg")
    run = convert(tmp_path, src, "pdf")
    with pikepdf.open(run.outputs[0]) as pdf:
        assert not pdf.pages[0].images


def test_repeat_runs_give_identical_bytes(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    for to in ("jpg", "pdf", "webp"):
        a = convert(tmp_path / f"{to}1", src, to).outputs[0].read_bytes()
        b = convert(tmp_path / f"{to}2", src, to).outputs[0].read_bytes()
        assert a == b, to


def test_unsupported_pair_fails_validation(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    run = convert(tmp_path, src, "png")
    assert run.errors and run.errors[0].param == "to"


@pytest.mark.parametrize("bad", ["corrupt-png", "truncated-png", "corrupt-gif"])
def test_broken_images_fail_cleanly(tmp_path: Path, bad: str) -> None:
    src = files.build(bad, tmp_path / "corpus")
    with pytest.raises(Exception) as e:
        convert(tmp_path, src, "jpg")
    # A garbage header can claim an absurd size: refusing it is also correct.
    assert getattr(e.value, "code", None) in ("E_INPUT_CORRUPT", "E_LIMIT_EXCEEDED")
