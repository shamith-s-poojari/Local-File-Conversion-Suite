"""Choosing the Ogg Vorbis encoder for the FFmpeg build at hand."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_sdk import media
from converter_sdk.errors import AdapterError


@pytest.fixture
def ffmpeg_with(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    def use(encoders: set[str]) -> None:
        # Absolute on every OS: "/opt/..." has no drive on Windows.
        monkeypatch.setenv("LFCS_BIN_FFMPEG", str(tmp_path / "ffmpeg"))
        monkeypatch.setattr(media, "_encoders", lambda _exe: frozenset(encoders))

    return use


def test_libvorbis_is_used_when_the_build_has_it(ffmpeg_with) -> None:
    ffmpeg_with({"libvorbis", "vorbis", "aac"})
    assert media.audio_codec("ogg") == ["-c:a", "libvorbis"]
    assert media.audio_encode_args("ogg", 128)[:2] == ["-c:a", "libvorbis"]


def test_a_build_without_libvorbis_cant_write_ogg(ffmpeg_with) -> None:
    # Homebrew's FFmpeg 9 has no libvorbis; its own Vorbis encoder crashes.
    ffmpeg_with({"vorbis", "aac", "libopus"})
    with pytest.raises(AdapterError) as e:
        media.audio_codec("ogg")
    assert e.value.code == "E_INSTALL_CORRUPT"
    with pytest.raises(AdapterError):
        media.audio_encode_args("ogg", 128)
    assert media.audio_codec("mp3") == ["-c:a", "libmp3lame"]


def test_an_unanswered_question_keeps_libvorbis(ffmpeg_with) -> None:
    ffmpeg_with(set())
    assert media.audio_codec("ogg") == ["-c:a", "libvorbis"]


def test_other_formats_are_unchanged(ffmpeg_with) -> None:
    ffmpeg_with({"vorbis"})
    assert media.audio_codec("mp3") == ["-c:a", "libmp3lame"]
    assert media.audio_codec("opus") == ["-c:a", "libopus"]


def test_the_encoder_list_is_parsed(monkeypatch: pytest.MonkeyPatch) -> None:
    listing = (
        "Encoders:\n V..... = Video\n ------\n"
        " A....D vorbis               Vorbis\n"
        " A....D libvorbis            libvorbis (codec vorbis)\n"
    )

    class Done:
        stdout = listing

    monkeypatch.setattr(media.subprocess, "run", lambda *_a, **_k: Done())
    media._encoders.cache_clear()
    try:
        found = media._encoders("/x/ffmpeg")
    finally:
        media._encoders.cache_clear()
    assert {"vorbis", "libvorbis"} <= found
