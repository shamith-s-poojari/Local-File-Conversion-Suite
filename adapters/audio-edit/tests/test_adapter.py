"""Acceptance tests for audio-edit (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


def test_each_file_keeps_its_format(tmp_path: Path) -> None:
    a = files.wav_tone(tmp_path / "a.wav", seconds=2.0)
    b = files.audio_file(tmp_path / "b.mp3", "mp3", seconds=2.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"volume": -6, "fileTitles": ["a", "b"]})
    assert not r.errors
    assert r.names == ["a-edited.wav", "b-edited.mp3"]
    codecs = [probe(o)["streams"][0]["codec_name"] for o in r.outputs]
    assert codecs == ["pcm_s16le", "mp3"]


def test_speed_changes_the_length(tmp_path: Path) -> None:
    src = files.wav_tone(tmp_path / "a.wav", seconds=4.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"speed": 2})
    assert abs(float(probe(r.outputs[0])["format"]["duration"]) - 2.0) < 0.1
    assert int(probe(r.outputs[0])["streams"][0]["sample_rate"]) == 44100


def test_nothing_to_change_is_refused(tmp_path: Path) -> None:
    src = files.wav_tone(tmp_path / "a.wav", seconds=1.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {})
    assert r.errors
