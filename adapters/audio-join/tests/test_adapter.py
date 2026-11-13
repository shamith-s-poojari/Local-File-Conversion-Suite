"""Acceptance tests for audio-join (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]

pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


def _seconds(path: Path) -> float:
    return float(probe(path)["format"]["duration"])


def test_lengths_add_up_with_gaps(tmp_path: Path) -> None:
    a = files.wav_tone(tmp_path / "a.wav", seconds=2.0, rate=44100, channels=1)
    b = files.audio_file(tmp_path / "b.mp3", "mp3", seconds=3.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"to": "flac", "gap": 1})
    assert not r.errors
    assert r.names == ["joined.flac"]
    assert abs(_seconds(r.outputs[0]) - 6.0) < 0.15
    stream = probe(r.outputs[0])["streams"][0]
    assert stream["codec_name"] == "flac"
    assert int(stream["channels"]) == 2  # b is stereo


def test_resampling_is_reported(tmp_path: Path) -> None:
    a = files.wav_tone(tmp_path / "a.wav", seconds=1.0, rate=22050)
    b = files.wav_tone(tmp_path / "b.wav", seconds=1.0, rate=44100)
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"to": "mp3", "bitrate": 128})
    assert "AUDIO_RESAMPLED" in r.warnings
    assert int(probe(r.outputs[0])["streams"][0]["sample_rate"]) == 44100
