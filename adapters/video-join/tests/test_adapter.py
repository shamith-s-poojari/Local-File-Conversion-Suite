"""Acceptance tests for video-join (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.skip_ffmpeg(), reason="FFmpeg isn't installed")


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


def test_concat_list_escapes_quotes() -> None:
    """P10: a quote in a path must not end the quoted name early."""
    from converter_conformance.harness import load_adapter_class  # noqa: PLC0415

    module = __import__("sys").modules[load_adapter_class(ADAPTER).__module__]
    listing = module.concat_list([Path("/jobs/it's/in/0.mp4"), Path("/jobs/a b/in/1.mp4")])
    # Paths are resolved, so Windows adds a drive letter before /jobs.
    first, second = listing.splitlines()
    assert first.startswith("file '") and first.endswith("/jobs/it'\\''s/in/0.mp4'")
    assert second.startswith("file '") and second.endswith("/jobs/a b/in/1.mp4'")
    assert listing.endswith("\n") and listing.count("\n") == 2
    with pytest.raises(Exception) as e:  # noqa: PT011 - AdapterError
        module.concat_list([Path("/jobs/a\nb/in/0.mp4")])
    assert e.value.code == "E_INTERNAL"


def test_join_works_in_a_folder_with_a_quote_in_its_name(tmp_path: Path) -> None:
    a = files.video_file(tmp_path / "a.mp4", "mp4", seconds=1.0)
    b = files.video_file(tmp_path / "b.mp4", "mp4", seconds=1.0)
    r = run_in_process(ADAPTER, tmp_path / "it's a job", [a, b], {"to": "mp4"})
    assert not r.errors
    assert "REENCODED" not in r.warnings  # the stream-copy path read the list
    assert abs(_length(r.outputs[0]) - 2.0) < 0.3
