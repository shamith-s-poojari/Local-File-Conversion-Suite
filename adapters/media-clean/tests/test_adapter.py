"""Acceptance tests for media-clean: details come out, sound and picture stay."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import context, run_in_process
from converter_sdk import reveal
from converter_sdk.media import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="needs ffmpeg")

TAGS = [
    "-metadata", "location=+37.7749-122.4194+010.0/",
    "-metadata", "artist=Ada", "-metadata", "title=Private title",
    "-metadata", "creation_time=2024-05-01T10:00:00Z",
    "-metadata", "comment=private",
]  # fmt: skip


def make(path: Path, *args: str) -> Path:
    ffmpeg = str(files.ffmpeg())
    subprocess.run(  # noqa: S603
        [ffmpeg, "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
         *args, str(path)],
        check=True, capture_output=True,
    )  # fmt: skip
    return path


def probed(tmp_path: Path, path: Path):
    (tmp_path / "pj").mkdir(exist_ok=True)
    ctx, _ = context(tmp_path / "pj", [])
    return probe(ctx, path)


def test_mp4_loses_location_and_keeps_streams(tmp_path: Path) -> None:
    ffmpeg = str(files.ffmpeg())
    src = tmp_path / "in.mp4"
    subprocess.run(  # noqa: S603
        [ffmpeg, "-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=64x64:d=1",
         "-f", "lavfi", "-i", "sine=frequency=440:duration=1", *TAGS,
         "-metadata:s:v:0", "handler_name=Cam", "-c:v", "libx264", "-pix_fmt", "yuv420p",
         "-c:a", "aac", "-shortest", str(src)],
        check=True, capture_output=True,
    )  # fmt: skip
    before = reveal.media_findings(probed(tmp_path, src))
    assert {"location", "author", "dates"} <= {f.kind for f in before}
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"fileTitles": ["clip"]})
    assert not r.errors
    assert r.names == ["clip-clean.mp4"]
    assert "DETAILS_REMOVED" in r.warnings
    out = probed(tmp_path, r.outputs[0])
    kinds = {f.kind for f in reveal.media_findings(out)}
    assert not kinds & {"location", "device", "author", "dates", "tags"}
    assert [s["codec_name"] for s in out["streams"]] == [
        s["codec_name"] for s in probed(tmp_path, src)["streams"]
    ]
    assert b"Private title" not in r.outputs[0].read_bytes()


@pytest.mark.parametrize("fmt", ["mp3", "flac", "ogg", "m4a", "wav"])
def test_audio_formats_are_cleaned(tmp_path: Path, fmt: str) -> None:
    src = files.audio_file(tmp_path / f"in.{fmt}", fmt)
    tagged = tmp_path / f"tagged.{fmt}"
    subprocess.run(  # noqa: S603
        [str(files.ffmpeg()), "-v", "error", "-y", "-i", str(src), "-c", "copy",
         "-metadata", "artist=Ada", "-metadata", "title=Private title", str(tagged)],
        check=True, capture_output=True,
    )  # fmt: skip
    assert "author" in {f.kind for f in reveal.media_findings(probed(tmp_path, tagged))}
    r = run_in_process(ADAPTER, tmp_path / "job", [tagged], {})
    assert not r.errors
    assert r.names == [f"file-1-clean.{fmt}"]
    kinds = {f.kind for f in reveal.media_findings(probed(tmp_path, r.outputs[0]))}
    assert not kinds & {"location", "device", "author", "dates", "tags"}


def test_cover_art_is_kept_unless_asked(tmp_path: Path) -> None:
    from PIL import Image

    cover = tmp_path / "cover.png"
    Image.new("RGB", (32, 32), (0, 128, 0)).save(cover)
    src = tmp_path / "in.mp3"
    subprocess.run(  # noqa: S603
        [str(files.ffmpeg()), "-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=1",
         "-i", str(cover), "-map", "0", "-map", "1", "-c:a", "libmp3lame", "-c:v", "copy",
         "-metadata:s:v", "title=Album cover", "-disposition:v", "attached_pic", str(src)],
        check=True, capture_output=True,
    )  # fmt: skip
    kept = run_in_process(ADAPTER, tmp_path / "a", [src], {})
    kept_info = probed(tmp_path, kept.outputs[0])
    assert "Cover picture" in [f.detail for f in reveal.media_findings(kept_info)]
    gone = run_in_process(ADAPTER, tmp_path / "b", [src], {"removeCoverArt": True})
    gone_info = probed(tmp_path, gone.outputs[0])
    assert [s["codec_type"] for s in gone_info["streams"]] == ["audio"]
