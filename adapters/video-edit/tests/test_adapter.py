"""Acceptance tests for video-edit (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


def _video(path: Path) -> dict:
    return next(s for s in probe(path)["streams"] if s["codec_type"] == "video")


def test_rotation_turns_the_frame_and_keeps_the_format(tmp_path: Path) -> None:
    a = files.video_file(tmp_path / "a.mp4", "mp4", seconds=1.0, size="320x240")
    b = files.video_file(tmp_path / "b.mkv", "mkv", seconds=1.0, size="320x240")
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"rotate": 90, "fileTitles": ["a", "b"]})
    assert not r.errors
    assert r.names == ["a-edited.mp4", "b-edited.mkv"]
    for out in r.outputs:
        v = _video(out)
        assert (v["width"], v["height"]) == (240, 320)


def test_speed_halves_the_length(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=2.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"speed": 2})
    assert abs(float(probe(r.outputs[0])["format"]["duration"]) - 1.0) < 0.2


def test_mute_removes_the_sound(tmp_path: Path) -> None:
    src = files.video_file(tmp_path / "a.mp4", "mp4", seconds=1.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"removeAudio": True})
    assert all(s["codec_type"] != "audio" for s in probe(r.outputs[0])["streams"])
