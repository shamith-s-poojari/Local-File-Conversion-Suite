"""FFmpeg for audio and video adapters (ADR 0003).

``probe`` reads a file's streams with ffprobe; ``ffmpeg`` runs a conversion
with progress and cancellation. Both only read local files: inputs are
opened with ``-protocol_whitelist file``, and the sandbox has no network.
"""

from __future__ import annotations

import json
import os
from collections.abc import Sequence
from pathlib import Path
from typing import Any

from .context import JobContext
from .engines import binary, run
from .errors import AdapterError
from .protocol import MediaInfo

CORRUPT_MARKERS = (
    "invalid data found",
    "could not find codec parameters",
    "moov atom not found",
    "error while decoding",
    "end of file",
    "header missing",
    "no such file",
    "does not contain any stream",
    "invalid argument",
    "unknown format",
    "failed to read",
    "truncat",
)

LOSSLESS_AUDIO = {"wav", "flac", "aiff"}
AUDIO_FORMATS = ["mp3", "wav", "flac", "ogg", "opus", "m4a", "aac", "aiff"]
VIDEO_FORMATS = ["mp4", "mov", "mkv", "webm", "avi"]

BITEXACT = ["-fflags", "+bitexact", "-flags:v", "+bitexact", "-flags:a", "+bitexact"]
"""Encoders and muxers leave out version strings and random IDs, so repeat runs match."""

AUDIO_CODEC = {
    "mp3": ["-c:a", "libmp3lame"],
    "ogg": ["-c:a", "libvorbis"],
    "opus": ["-c:a", "libopus"],
    "m4a": ["-c:a", "aac"],
    "aac": ["-c:a", "aac"],
    "flac": ["-c:a", "flac"],
    "wav": ["-c:a", "pcm_s16le"],
    "aiff": ["-c:a", "pcm_s16be"],
}
"""The encoder for each audio format (16-bit PCM for WAV and AIFF)."""
MUXER = {
    "mp4": "mp4", "mov": "mov", "mkv": "matroska", "webm": "webm", "avi": "avi", "gif": "gif",
    "mp3": "mp3", "ogg": "ogg", "opus": "opus", "m4a": "ipod", "aac": "adts", "flac": "flac",
    "wav": "wav", "aiff": "aiff",
}  # fmt: skip
OPUS_RATES = {48000, 24000, 16000, 12000, 8000}


def audio_encode_args(to: str, bitrate_kbps: int = 192, rate: int = 0) -> list[str]:
    """Encoder, bitrate (lossy formats) and a sample rate the format takes."""
    args = list(AUDIO_CODEC[to])
    if to not in LOSSLESS_AUDIO:
        args += ["-b:a", f"{bitrate_kbps}k"]
    if to == "opus" and rate not in OPUS_RATES:
        rate = 48000
    if rate:
        args += ["-ar", str(rate)]
    return args


def video_encode_args(to: str) -> list[str]:
    """High-quality video and audio encoders for a re-encode into ``to``
    (the tools re-encode only when they must, so they keep quality high)."""
    if to == "webm":
        return ["-c:v", "libvpx-vp9", "-crf", "30", "-b:v", "0", "-row-mt", "1",
                "-deadline", "good", "-cpu-used", "4", "-pix_fmt", "yuv420p",
                "-c:a", "libopus", "-b:a", "128k", "-ar", "48000"]  # fmt: skip
    if to == "avi":
        return ["-c:v", "mpeg4", "-vtag", "xvid", "-q:v", "2", "-pix_fmt", "yuv420p",
                "-c:a", "libmp3lame", "-b:a", "192k"]  # fmt: skip
    args = ["-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "192k"]  # fmt: skip
    if to in ("mp4", "mov"):
        args += ["-movflags", "+faststart"]
    return args


def frame_rate(stream: dict[str, Any]) -> float:
    """A video stream's average frame rate, or 25 if it doesn't say."""
    for key in ("avg_frame_rate", "r_frame_rate"):
        value = str(stream.get(key) or "")
        if "/" in value:
            num, _, den = value.partition("/")
            try:
                if float(den) > 0 and float(num) > 0:
                    return float(num) / float(den)
            except ValueError:
                continue
    return 25.0


def atempo(speed: float) -> str:
    """An ``atempo`` chain for any speed (each stage takes 0.5 to 2)."""
    stages = []
    while speed < 0.5:
        stages.append("atempo=0.5")
        speed /= 0.5
    while speed > 2.0:
        stages.append("atempo=2.0")
        speed /= 2.0
    stages.append(f"atempo={speed:g}")
    return ",".join(stages)


def has_extra_streams(info: dict[str, Any]) -> bool:
    """Tracks beyond the first audio and video: subtitles, data, cover art,
    more audio. Tools that keep only the first of each say so."""
    streams = info.get("streams", [])
    audio = audio_streams(info)
    video = video_streams(info)
    return len(streams) > min(len(audio), 1) + min(len(video), 1)


def probe(ctx: JobContext, path: Path) -> dict[str, Any]:
    """ffprobe's JSON for one input. Unreadable media is ``E_INPUT_CORRUPT``."""
    exe = binary("ffprobe")
    code, err = _run_capture(
        ctx,
        [
            exe, "-v", "error", "-protocol_whitelist", "file",
            "-print_format", "json", "-show_format", "-show_streams",
            os.fspath(path),
        ],
    )  # fmt: skip
    if code != 0:
        raise AdapterError("E_INPUT_CORRUPT")
    try:
        data = json.loads(err)
    except json.JSONDecodeError:
        raise AdapterError("E_INPUT_CORRUPT") from None
    if not data.get("streams"):
        raise AdapterError("E_INPUT_CORRUPT")
    return data


def _run_capture(ctx: JobContext, argv: list[Any]) -> tuple[int, str]:
    """Like ``engines.run``, but returns stdout instead of stderr."""
    lines: list[str] = []
    code, _ = run(ctx, argv, on_line=lines.append, timeout_s=60)
    return code, "\n".join(lines)


def audio_streams(info: dict[str, Any]) -> list[dict[str, Any]]:
    return [s for s in info.get("streams", []) if s.get("codec_type") == "audio"]


def video_streams(info: dict[str, Any]) -> list[dict[str, Any]]:
    """Real video, not an MP3's cover picture."""
    return [
        s
        for s in info.get("streams", [])
        if s.get("codec_type") == "video" and not (s.get("disposition") or {}).get("attached_pic")
    ]


def duration_s(info: dict[str, Any]) -> float:
    try:
        return max(0.0, float(info.get("format", {}).get("duration") or 0))
    except (TypeError, ValueError):
        return 0.0


def media_info(info: dict[str, Any], kind: str) -> MediaInfo:
    audio = audio_streams(info)
    video = video_streams(info)
    a = audio[0] if audio else {}
    v = video[0] if video else {}
    fps = None
    if v.get("avg_frame_rate") and "/" in v["avg_frame_rate"]:
        num, den = v["avg_frame_rate"].split("/")
        fps = round(int(num) / int(den), 2) if int(den or 0) else None
    bitrate = info.get("format", {}).get("bit_rate")
    return MediaInfo(
        kind=kind,  # type: ignore[arg-type]
        durationMs=int(duration_s(info) * 1000),
        width=int(v["width"]) if v.get("width") else None,
        height=int(v["height"]) if v.get("height") else None,
        videoCodec=str(v.get("codec_name"))[:64] if v else None,
        audioCodec=str(a.get("codec_name"))[:64] if a else None,
        sampleRate=int(a["sample_rate"]) if a.get("sample_rate") else None,
        channels=int(a["channels"]) if a.get("channels") else None,
        bitrateKbps=int(int(bitrate) / 1000) if bitrate and str(bitrate).isdigit() else None,
        fps=fps,
        hasAudio=bool(audio),
        hasVideo=bool(video),
    )


def parse_time(value: Any) -> float | None:
    """``"90"``, ``"1:30"`` or ``"0:01:30.5"`` → seconds. Empty → None."""
    if value is None or str(value).strip() == "":
        return None
    parts = str(value).strip().split(":")
    if len(parts) > 3:
        raise ValueError("time")
    seconds = 0.0
    for p in parts:
        if not p.replace(".", "", 1).isdigit():
            raise ValueError("time")
        seconds = seconds * 60 + float(p)
    return seconds


def ffmpeg(
    ctx: JobContext,
    inputs: Sequence[str | os.PathLike[str]],
    output_args: Sequence[str],
    out: Path,
    *,
    duration: float,
    stage: str,
    input_args: Sequence[str] = (),
) -> None:
    """Run one FFmpeg conversion, reporting progress in milliseconds of output."""
    exe = binary("ffmpeg")
    argv: list[Any] = [
        exe,
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-nostats",
        "-progress",
        "pipe:1",
    ]
    for i in inputs:
        argv += [*input_args, "-protocol_whitelist", "file", "-i", os.fspath(i)]
    argv += [*output_args, *BITEXACT, os.fspath(out)]
    total_ms = max(1, int(duration * 1000))

    def on_line(line: str) -> None:
        key, _, value = line.partition("=")
        if key in ("out_time_us", "out_time_ms") and value.strip().isdigit():
            done = min(total_ms, int(value) // 1000)
            ctx.progress(stage, done, total_ms)

    code, err = run(ctx, argv, on_line=on_line)
    if code != 0:
        out.unlink(missing_ok=True)
        lowered = err.lower()
        if any(m in lowered for m in CORRUPT_MARKERS):
            raise AdapterError("E_INPUT_CORRUPT")
        if "no space left" in lowered:
            raise AdapterError("E_RESOURCE_DISK")
        # Only the exit code: stderr can quote file names and metadata.
        raise AdapterError("E_INTERNAL", {"reason": "engineFailed", "exit": code})
    if not out.is_file() or out.stat().st_size == 0:
        raise AdapterError("E_OUTPUT_INVALID", {"reason": "empty"})
    ctx.progress(stage, total_ms, total_ms)


def render_waveform(ctx: JobContext, path: Path, out: Path, width: int, height: int) -> None:
    """A waveform picture of the whole file, for previews."""
    exe = binary("ffmpeg")
    code, _ = run(
        ctx,
        [
            exe, "-hide_banner", "-nostdin", "-y", "-loglevel", "error",
            "-protocol_whitelist", "file", "-i", os.fspath(path),
            "-filter_complex",
            "[0:a:0]aformat=channel_layouts=mono,"
            f"showwavespic=s={width}x{height}:colors=0x8c8880:scale=sqrt",
            "-frames:v", "1", os.fspath(out),
        ],
        timeout_s=60,
    )  # fmt: skip
    if code != 0 or not out.is_file():
        raise AdapterError("E_INPUT_CORRUPT")


def render_frame(ctx: JobContext, path: Path, out: Path, at_s: float, max_side: int) -> None:
    """One video frame (a poster), longest side at most ``max_side``."""
    exe = binary("ffmpeg")
    scale = f"scale='if(gt(iw,ih),min({max_side},iw),-2)':'if(gt(iw,ih),-2,min({max_side},ih))'"
    code, _ = run(
        ctx,
        [
            exe, "-hide_banner", "-nostdin", "-y", "-loglevel", "error",
            "-ss", f"{at_s:.3f}", "-protocol_whitelist", "file", "-i", os.fspath(path),
            "-map", "0:v:0", "-frames:v", "1", "-vf", scale, os.fspath(out),
        ],
        timeout_s=60,
    )  # fmt: skip
    if code != 0 or not out.is_file():
        raise AdapterError("E_INPUT_CORRUPT")
