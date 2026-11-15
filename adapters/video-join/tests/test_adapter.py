"""Acceptance tests for video-join (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


def _length(path: Path) -> float:
    return float(probe(path)["format"]["duration"])


def test_matching_clips_join_without_reencoding(tmp_path: Path) -> None:
    a = files.video_file(tmp_path / "a.mp4", "mp4", seconds=2.0)
    b = files.video_file(tmp_path / "b.mp4", "mp4", seconds=2.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"to": "mp4"})
    assert not r.errors
    assert r.names == ["joined.mp4"]
    assert "REENCODED" not in r.warnings
    assert abs(_length(r.outputs[0]) - 4.0) < 0.3


def test_different_clips_are_reencoded_into_the_first_frame(tmp_path: Path) -> None:
    a = files.video_file(tmp_path / "a.mp4", "mp4", seconds=1.0, size="320x240")
    b = files.video_file(tmp_path / "b.webm", "webm", seconds=1.0, size="160x160")
    c = files.silent_video(tmp_path / "c.mp4", seconds=1.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b, c], {"to": "mp4"})
    assert "REENCODED" in r.warnings
    streams = probe(r.outputs[0])["streams"]
    video = next(s for s in streams if s["codec_type"] == "video")
    assert (video["width"], video["height"]) == (320, 240)
    assert any(s["codec_type"] == "audio" for s in streams)
    assert abs(_length(r.outputs[0]) - 3.0) < 0.4
