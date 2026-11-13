"""Acceptance tests for video-convert (ADR 0003). Need FFmpeg."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml
from converter_conformance import files, validate
from converter_conformance.harness import context, load_adapter_class, make_job_dir, run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


@pytest.fixture(scope="module")
def sources(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    d = tmp_path_factory.mktemp("video")
    out = {f: files.video_file(d / f"clip.{f}", f) for f in files.VIDEO_FORMATS}
    out["gif"] = files.image_file(d / "clip.gif", "gif")
    out["silent"] = files.silent_video(d / "silent.mp4")
    return out


def convert(tmp_path: Path, src: Path, to: str, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["clip"], **params}
    )


def streams(path: Path, kind: str) -> list[dict]:
    return [s for s in validate.probe(path)["streams"] if s["codec_type"] == kind]


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize(("src", "to"), sorted(CLS.EDGES))
def test_every_pair(tmp_path: Path, sources, src: str, to: str) -> None:
    r = convert(tmp_path, sources[src], to)
    assert not r.errors, r.errors
    assert r.names == [f"clip.{to}"]
    validate.check(r.outputs[0], to)


def test_same_quality_copies_the_picture(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["mp4"], "mkv")
    out = streams(r.outputs[0], "video")[0]
    src = streams(sources["mp4"], "video")[0]
    # Copied, not re-encoded: same codec, profile and level.
    assert out["codec_name"] == "h264"
    assert (out["profile"], out["level"]) == (src["profile"], src["level"])


def test_resolution_only_shrinks_and_stays_even(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["mp4"], "webm", maxHeight=180)
    v = streams(r.outputs[0], "video")[0]
    assert v["height"] == 180 and v["width"] % 2 == 0
    r = convert(tmp_path / "b", sources["mp4"], "mov", maxHeight=1080, quality="high")
    assert streams(r.outputs[0], "video")[0]["height"] == 240


def test_remove_sound(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["mov"], "mp4", removeAudio=True)
    assert streams(r.outputs[0], "audio") == []


def test_trim(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["mp4"], "mkv", start="0.5", end="1.5")
    duration = float(validate.probe(r.outputs[0])["format"]["duration"])
    assert abs(duration - 1.0) < 0.15


def test_gif_options(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["mp4"], "gif", gifWidth=320, fps=8)
    with Image.open(r.outputs[0]) as img:
        assert img.width == 320 and img.n_frames == 16


def test_soundtrack_needs_sound(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["silent"], "mp3")
    assert r.errors and r.errors[0].param == "to"


def test_poster_frame_preview(tmp_path: Path, sources) -> None:
    job, rel = make_job_dir(tmp_path / "job", [sources["mp4"]])
    ctx, _ = context(job, rel)
    out = job / "out" / "p" / "1.png"
    page = CLS().render(ctx, ctx.inputs[0], 1, 48, out)
    assert (page.width_px, page.height_px) == (320, 240)


def test_inspect_reports_media_facts(tmp_path: Path, sources) -> None:
    job, rel = make_job_dir(tmp_path / "job", [sources["mp4"]])
    ctx, _ = context(job, rel)
    m = CLS().inspect(ctx, ctx.inputs[0]).media
    assert (m.width, m.height, m.hasAudio, m.videoCodec) == (320, 240, True, "h264")
    assert abs(m.durationMs - 2000) < 100
