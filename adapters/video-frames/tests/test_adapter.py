"""Acceptance tests for video-frames (ADR 0004)."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import load_adapter_class, run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


def test_frames_spread_evenly_and_named_by_time(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=4.0, size="320x240")
    r = run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": "jpg", "count": 4, "fileTitles": ["clip"]}
    )
    assert not r.errors
    assert r.names == ["clip-00m00s.jpg", "clip-00m01s.jpg", "clip-00m02s.jpg", "clip-00m03s.jpg"]
    with Image.open(r.outputs[0]) as img:
        assert img.format == "JPEG"
        assert img.size == (320, 240)


def test_frames_under_a_second_apart_are_named_by_tenths(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=2.0, size="160x120")
    r = run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": "jpg", "count": 5, "fileTitles": ["clip"]}
    )
    assert not r.errors
    assert r.names == [
        "clip-00m00.2s.jpg",
        "clip-00m00.6s.jpg",
        "clip-00m01.0s.jpg",
        "clip-00m01.4s.jpg",
        "clip-00m01.8s.jpg",
    ]


def test_stamp_labels() -> None:
    module = sys.modules[load_adapter_class(ADAPTER).__module__]
    stamp, stamps = module.stamp, module.stamps
    assert stamp(65.9) == "01m05s"
    assert stamp(3725.0) == "1h02m05s"
    assert stamp(1.45, tenths=True) == "00m01.4s"
    assert stamps([0.5, 1.5]) == ["00m00s", "00m01s"]
    assert stamps([0.2, 0.6]) == ["00m00.2s", "00m00.6s"]


def test_interval_and_width(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=3.0, size="640x480")
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [src],
        {"to": "png", "mode": "interval", "every": 1, "maxWidth": 640},
    )
    assert len(r.outputs) == 3
    with Image.open(r.outputs[-1]) as img:
        assert img.format == "PNG"
        assert img.width == 640


def test_sheet_is_one_picture_with_every_frame(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=4.0, size="320x240")
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [src],
        {"to": "png", "layout": "sheet", "count": 6, "fileTitles": ["clip"]},
    )
    assert not r.errors
    assert r.names == ["clip-sheet.png"]
    with Image.open(r.outputs[0]) as img:
        assert img.format == "PNG"
        # 6 frames: 3 columns of 4:3 cells over 2 rows, with captions and a title.
        assert img.width > 3 * 320
        assert img.height > 2 * 240


def test_sheet_in_jpg_and_by_interval(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=6.0, size="160x120")
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [src],
        {"to": "jpg", "layout": "sheet", "mode": "interval", "every": 1, "fileTitles": ["clip"]},
    )
    assert not r.errors
    assert r.names == ["clip-sheet.jpg"]
    with Image.open(r.outputs[0]) as img:
        assert img.format == "JPEG"


def test_sheet_frame_count_is_checked_up_front(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=4.0, size="160x120")
    few = run_in_process(
        ADAPTER, tmp_path / "a", [src], {"to": "jpg", "layout": "sheet", "count": 3}
    )
    assert few.errors and few.errors[0].param == "count"
    many = run_in_process(
        ADAPTER, tmp_path / "b", [src], {"to": "jpg", "layout": "sheet", "count": 50}
    )
    assert many.errors and many.errors[0].param == "count"
    one_second = run_in_process(
        ADAPTER,
        tmp_path / "c",
        [src],
        {"to": "jpg", "layout": "sheet", "mode": "interval", "every": 30},
    )
    assert one_second.errors and one_second.errors[0].param == "every"
