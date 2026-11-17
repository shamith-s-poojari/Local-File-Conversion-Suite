"""Acceptance tests for video-edit (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.skip_ffmpeg(), reason="FFmpeg isn't installed")


def _video(path: Path) -> dict:
    return next(s for s in probe(path)["streams"] if s["codec_type"] == "video")


def test_rotation_turns_the_frame_and_keeps_the_format(tmp_path: Path) -> None:
    a = files.video_file(tmp_path / "a.mp4", "mp4", seconds=1.0, size="320x240")
    b = files.video_file(tmp_path / "b.mkv", "mkv", seconds=1.0, size="320x240")
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"rotate": 90, "fileTitles": ["a", "b"]})
    assert not r.errors
    assert r.names == ["a-edited.mp4", "b-edited.mkv"]
    assert r.sources == [0, 1]
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


def _module():
    import sys  # noqa: PLC0415

    from converter_conformance.harness import load_adapter_class  # noqa: PLC0415

    return sys.modules[load_adapter_class(ADAPTER).__module__]


@pytest.mark.parametrize(
    "bad",
    [{"rotate": 45}, {"rotate": "90"}, {"flip": "diagonal"}, {"speed": 100}, {"speed": True},
     {"speed": 0.0001}, {"removeAudio": "yes"}],
)  # fmt: skip
def test_params_are_rechecked_in_the_adapter(tmp_path: Path, bad: dict) -> None:
    """P17: out-of-range values are refused here, before FFmpeg, whatever the schema said."""
    from converter_conformance.harness import context, make_job_dir  # noqa: PLC0415
    from converter_sdk import AdapterError  # noqa: PLC0415

    module = _module()
    job, rel = make_job_dir(tmp_path / "job", [])
    ctx, _ = context(job, rel)
    adapter = module.VideoEditAdapter()
    report = adapter.validate_input(ctx, {"rotate": 90, **bad})
    assert [e.code for e in report.errors] == ["E_INTERNAL"]
    assert report.errors[0].param == next(iter(bad))
    with pytest.raises(AdapterError):
        adapter.execute_conversion(ctx, {"rotate": 90, **bad})


def test_allowed_values_match_the_schema() -> None:
    import json  # noqa: PLC0415

    props = json.loads((ADAPTER / "params.schema.json").read_text("utf-8"))["properties"]
    module = _module()
    assert tuple(props["rotate"]["enum"]) == module.ROTATIONS
    assert tuple(props["flip"]["enum"]) == module.FLIPS
    assert tuple(props["speed"]["enum"]) == module.SPEEDS
