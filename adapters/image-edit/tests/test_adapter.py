"""Acceptance tests for image-edit (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

from converter_conformance import files
from converter_conformance.harness import run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]


def test_each_file_keeps_its_format_and_turns(tmp_path: Path) -> None:
    srcs = [files.image_file(tmp_path / f"in.{f}", f) for f in ("png", "jpg")]
    r = run_in_process(ADAPTER, tmp_path / "job", srcs, {"rotate": 90, "fileTitles": ["a", "b"]})
    assert not r.errors
    assert r.names == ["a-edited.png", "b-edited.jpg"]
    with Image.open(r.outputs[0]) as png:
        assert png.format == "PNG"
        assert png.size == (200, 320)  # 320×200 turned a quarter
    with Image.open(r.outputs[1]) as jpg:
        assert jpg.format == "JPEG"
        # The test JPEG is 200×320 once upright, so a quarter turn gives 320×200.
        assert jpg.size == (320, 200)


def test_flip_mirrors_pixels(tmp_path: Path) -> None:
    src = tmp_path / "in.png"
    img = Image.new("RGB", (4, 2), (0, 0, 0))
    img.putpixel((0, 0), (255, 0, 0))
    img.save(src)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"flip": "horizontal"})
    with Image.open(r.outputs[0]) as out:
        assert out.getpixel((3, 0)) == (255, 0, 0)
        assert out.getpixel((0, 0)) == (0, 0, 0)


def test_fit_never_enlarges_and_percent_scales(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.png", "png")
    fit = run_in_process(
        ADAPTER, tmp_path / "a", [src], {"resize": "fit", "width": 1000, "height": 1000}
    )
    with Image.open(fit.outputs[0]) as out:
        assert out.size == (320, 200)
    small = run_in_process(
        ADAPTER, tmp_path / "b", [src], {"resize": "fit", "width": 160, "height": 160}
    )
    with Image.open(small.outputs[0]) as out:
        assert out.size == (160, 100)
    assert "IMAGE_DOWNSCALED" in small.warnings
    half = run_in_process(ADAPTER, tmp_path / "c", [src], {"resize": "percent", "percent": 50})
    with Image.open(half.outputs[0]) as out:
        assert out.size == (160, 100)


def test_animation_keeps_every_frame(tmp_path: Path) -> None:
    src = tmp_path / "in.gif"
    frames = [Image.new("RGB", (40, 20), c) for c in ("red", "white", "blue")]
    frames[0].save(src, save_all=True, append_images=frames[1:], duration=80, loop=0)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"rotate": 180, "grayscale": True})
    with Image.open(r.outputs[0]) as out:
        assert out.format == "GIF"
        assert out.n_frames == 3


def test_metadata_can_be_removed(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.jpg", "jpg")
    kept = run_in_process(ADAPTER, tmp_path / "a", [src], {"grayscale": True})
    gone = run_in_process(ADAPTER, tmp_path / "b", [src], {"stripMetadata": True})
    with Image.open(kept.outputs[0]) as a, Image.open(gone.outputs[0]) as b:
        assert len(a.getexif()) > 0
        assert len(b.getexif()) == 0
