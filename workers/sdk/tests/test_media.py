"""Choosing the Ogg Vorbis encoder for the FFmpeg build at hand."""

from __future__ import annotations

import pytest
from converter_sdk import media


@pytest.fixture
def ffmpeg_with(monkeypatch: pytest.MonkeyPatch):
    def use(encoders: set[str]) -> None:
        monkeypatch.setenv("LFCS_BIN_FFMPEG", "/opt/ffmpeg/bin/ffmpeg")
        monkeypatch.setattr(media, "_encoders", lambda _exe: frozenset(encoders))

    return use


def test_libvorbis_is_used_when_the_build_has_it(ffmpeg_with) -> None:
    ffmpeg_with({"libvorbis", "vorbis", "aac"})
    assert media.audio_codec("ogg") == ["-c:a", "libvorbis"]
    assert media.audio_encode_args("ogg", 128)[:2] == ["-c:a", "libvorbis"]


def test_a_build_without_libvorbis_uses_ffmpegs_own_encoder(ffmpeg_with) -> None:
    # Homebrew's FFmpeg 9 has no libvorbis.
    ffmpeg_with({"vorbis", "aac", "libopus"})
    assert media.audio_codec("ogg") == media.VORBIS_BUILTIN
    assert media.audio_encode_args("ogg", 128)[: len(media.VORBIS_BUILTIN)] == media.VORBIS_BUILTIN


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
