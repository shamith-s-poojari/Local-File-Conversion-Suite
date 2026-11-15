"""Acceptance tests for audio-convert (ADR 0003). Need FFmpeg."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
import yaml
from converter_conformance import files, validate
from converter_conformance.harness import load_adapter_class, run_in_process

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)
pytestmark = pytest.mark.skipif(files.ffmpeg() is None, reason="FFmpeg isn't installed")


@pytest.fixture(scope="module")
def sources(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    d = tmp_path_factory.mktemp("audio")
    return {f: files.audio_file(d / f"song.{f}", f) for f in files.AUDIO_FORMATS}


def convert(tmp_path: Path, src: Path, to: str, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["song"], **params}
    )


def stream(path: Path) -> dict:
    return next(s for s in validate.probe(path)["streams"] if s["codec_type"] == "audio")


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize(("src", "to"), sorted(CLS.EDGES))
def test_every_pair(tmp_path: Path, sources, src: str, to: str) -> None:
    r = convert(tmp_path, sources[src], to)
    assert not r.errors, r.errors
    assert r.names == [f"song.{to}"]
    validate.check(r.outputs[0], to)
    info = validate.probe(r.outputs[0])
    assert abs(float(info["format"]["duration"]) - 3.0) < 0.2


def test_tags_survive(tmp_path: Path) -> None:
    src = tmp_path / "tagged.mp3"
    subprocess.run(  # noqa: S603
        [files.ffmpeg(), "-loglevel", "error", "-i", str(files.wav_tone(tmp_path / "t.wav", 1)),
         "-metadata", "title=Morning", "-metadata", "artist=Ines", str(src)],
        check=True,
    )  # fmt: skip
    r = convert(tmp_path, src, "flac")
    tags = {k.lower(): v for k, v in validate.probe(r.outputs[0])["format"].get("tags", {}).items()}
    assert tags.get("title") == "Morning" and tags.get("artist") == "Ines"


def test_trim_and_mono(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["wav"], "mp3", start="0:01", end="2", channels=1)
    info = validate.probe(r.outputs[0])
    assert abs(float(info["format"]["duration"]) - 1.0) < 0.15
    assert stream(r.outputs[0])["channels"] == 1


def test_bitrate_is_used(tmp_path: Path, sources) -> None:
    low = convert(tmp_path / "a", sources["wav"], "mp3", bitrate=96).outputs[0].stat().st_size
    high = convert(tmp_path / "b", sources["wav"], "mp3", bitrate=320).outputs[0].stat().st_size
    assert high > low * 2.5


def test_24_bit_stays_24_bit(tmp_path: Path) -> None:
    src = tmp_path / "deep.flac"
    subprocess.run(  # noqa: S603
        [files.ffmpeg(), "-loglevel", "error", "-f", "lavfi", "-i", "sine=f=440:d=1",
         "-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24", str(src)],
        check=True,
    )  # fmt: skip
    r = convert(tmp_path, src, "wav")
    assert stream(r.outputs[0])["codec_name"] == "pcm_s24le"


def test_aac_into_m4a_is_not_reencoded(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["aac"], "m4a")

    def pcm(p: Path) -> bytes:
        return subprocess.run(  # noqa: S603
            [files.ffmpeg(), "-v", "error", "-i", str(p), "-map", "0:a", "-f", "s16le", "-"],
            capture_output=True,
            check=True,
        ).stdout

    # Same packets decode to the same samples; a re-encode would not.
    assert pcm(sources["aac"]) == pcm(r.outputs[0])


def test_opus_from_44k_is_resampled(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["wav"], "opus")
    assert "AUDIO_RESAMPLED" in r.warnings
    validate.check(r.outputs[0], "opus")


def test_waveform_preview(tmp_path: Path, sources) -> None:
    from converter_conformance.harness import context, make_job_dir
    from PIL import Image

    job, rel = make_job_dir(tmp_path / "job", [sources["mp3"]])
    ctx, _ = context(job, rel)
    out = job / "out" / "p" / "1.png"
    page = CLS().render(ctx, ctx.inputs[0], 1, 72, out)
    with Image.open(out) as img:
        assert img.size == (page.width_px, page.height_px) == (576, 144)


def test_bad_times_are_param_errors(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["wav"], "mp3", start="0:02", end="0:01")
    assert r.errors and r.errors[0].param == "end"


def test_repeat_runs_are_identical(tmp_path: Path, sources) -> None:
    for to in ("mp3", "flac", "ogg", "m4a"):
        a = convert(tmp_path / f"{to}1", sources["wav"], to).outputs[0].read_bytes()
        b = convert(tmp_path / f"{to}2", sources["wav"], to).outputs[0].read_bytes()
        assert a == b, to
