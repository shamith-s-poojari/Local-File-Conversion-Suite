"""Acceptance tests for image-compress (ADR 0004)."""

from __future__ import annotations

import os
from pathlib import Path

from converter_conformance import files
from converter_conformance.harness import run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]


def _noisy(path: Path, side: int, fmt: str) -> Path:
    """A photo-like image that compresses badly, so it starts big."""
    img = Image.frombytes("RGB", (side, side), os.urandom(side * side * 3))
    img = img.resize((side * 2, side * 2), Image.Resampling.BILINEAR)
    img.save(path, {"jpg": "JPEG", "png": "PNG", "webp": "WEBP"}[fmt], quality=98)
    return path


def test_lossy_images_get_under_the_target_in_their_format(tmp_path: Path) -> None:
    srcs = [_noisy(tmp_path / f"in.{f}", 600, f) for f in ("jpg", "webp")]
    assert all(s.stat().st_size > 200_000 for s in srcs)
    r = run_in_process(ADAPTER, tmp_path / "job", srcs, {"targetKb": 200, "fileTitles": ["a", "b"]})
    assert not r.errors
    assert r.names == ["a-compressed.jpg", "b-compressed.webp"]
    assert r.sources == [0, 1]
    for out, fmt in zip(r.outputs, ("JPEG", "WEBP"), strict=True):
        assert out.stat().st_size <= 200_000
        with Image.open(out) as img:
            assert img.format == fmt


def test_png_reduces_colours_then_shrinks(tmp_path: Path) -> None:
    src = _noisy(tmp_path / "in.png", 700, "png")
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"targetKb": 100})
    assert r.outputs[0].stat().st_size <= 100_000
    assert "COLORS_REDUCED" in r.warnings
    with Image.open(r.outputs[0]) as img:
        assert img.format == "PNG"


def test_small_files_are_kept_byte_for_byte(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"targetKb": 2000})
    assert r.outputs[0].read_bytes() == src.read_bytes()
    assert r.warnings == ["KEPT_ORIGINAL"]


def test_camera_details_are_removed(tmp_path: Path) -> None:
    src = _noisy(tmp_path / "in.jpg", 500, "jpg")
    exif = Image.Exif()
    exif[0x010F] = "Fixture Camera"
    with Image.open(src) as img:
        img.save(src, "JPEG", quality=98, exif=exif.tobytes())
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"targetKb": 100})
    with Image.open(r.outputs[0]) as img:
        assert len(img.getexif()) == 0
    assert "METADATA_DROPPED" in r.warnings
