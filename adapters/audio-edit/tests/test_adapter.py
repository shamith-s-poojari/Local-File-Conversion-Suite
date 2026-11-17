"""Acceptance tests for audio-edit (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import run_in_process
from converter_conformance.validate import probe

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(files.skip_ffmpeg(), reason="FFmpeg isn't installed")


def test_each_file_keeps_its_format(tmp_path: Path) -> None:
    a = files.wav_tone(tmp_path / "a.wav", seconds=2.0)
    b = files.audio_file(tmp_path / "b.mp3", "mp3", seconds=2.0)
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"volume": -6, "fileTitles": ["a", "b"]})
    assert not r.errors
    assert r.names == ["a-edited.wav", "b-edited.mp3"]
    assert r.sources == [0, 1]
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


def _module():
    import sys  # noqa: PLC0415

    from converter_conformance.harness import load_adapter_class  # noqa: PLC0415

    return sys.modules[load_adapter_class(ADAPTER).__module__]


@pytest.mark.parametrize(
    "bad",
    [{"volume": 40}, {"volume": "3"}, {"fadeIn": 3600}, {"fadeOut": -1}, {"speed": 8},
     {"speed": False}, {"normalize": 1}],
)  # fmt: skip
def test_params_are_rechecked_in_the_adapter(tmp_path: Path, bad: dict) -> None:
    """P17: out-of-range values are refused here, before FFmpeg, whatever the schema said."""
    from converter_conformance.harness import context, make_job_dir  # noqa: PLC0415
    from converter_sdk import AdapterError  # noqa: PLC0415

    module = _module()
    job, rel = make_job_dir(tmp_path / "job", [])
    ctx, _ = context(job, rel)
    adapter = module.AudioEditAdapter()
    params = {"volume": 3, **bad}
    report = adapter.validate_input(ctx, params)
    assert [e.code for e in report.errors] == ["E_INTERNAL"]
    assert report.errors[0].param == next(iter(bad))
    with pytest.raises(AdapterError):
        adapter.execute_conversion(ctx, params)


def test_allowed_values_match_the_schema() -> None:
    import json  # noqa: PLC0415

    props = json.loads((ADAPTER / "params.schema.json").read_text("utf-8"))["properties"]
    module = _module()
    assert tuple(props["volume"]["enum"]) == module.VOLUMES
    assert tuple(props["fadeIn"]["enum"]) == module.FADES
    assert tuple(props["speed"]["enum"]) == module.SPEEDS
