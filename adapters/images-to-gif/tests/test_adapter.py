"""Acceptance tests for images-to-gif (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

from converter_conformance import files
from converter_conformance.harness import run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]


def test_frames_follow_order_size_and_timing(tmp_path: Path) -> None:
    srcs = [files.image_file(tmp_path / f"in.{f}", f) for f in ("png", "jpg", "gif")]
    r = run_in_process(ADAPTER, tmp_path / "job", srcs, {"frameMs": 1000, "width": 320})
    assert not r.errors
    assert r.names == ["animation.gif"]
    with Image.open(r.outputs[0]) as gif:
        assert gif.format == "GIF"
        assert gif.n_frames == 3
        assert gif.size == (320, 200)  # the first picture is 320×200
        assert gif.info["duration"] == 1000
        assert gif.info.get("loop") == 0
    assert "ANIMATION_FLATTENED" in r.warnings  # the GIF input was animated


def test_play_once(tmp_path: Path) -> None:
    srcs = [files.image_file(tmp_path / f"in.{f}", f) for f in ("png", "bmp")]
    r = run_in_process(ADAPTER, tmp_path / "job", srcs, {"loop": False})
    with Image.open(r.outputs[0]) as gif:
        assert "loop" not in gif.info
