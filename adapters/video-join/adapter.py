"""video-join: clips, in the chosen order, as one video (ADR 0004).

When every clip has the same picture (codec, size, pixel format, frame
rate) and sound (codec, sample rate, channels), and the chosen container
can hold them, the clips are joined without re-encoding. Otherwise each is
fitted into the first clip's frame (letterboxed where the shapes differ),
brought to its frame rate, given silence where it has no sound, and the
whole is encoded once (``REENCODED``)."""

from __future__ import annotations

from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import issue
from converter_sdk.media import (
    MUXER,
    audio_streams,
    duration_s,
    ffmpeg,
    frame_rate,
    has_extra_streams,
    probe,
    video_encode_args,
    video_streams,
)

MB = 1024 * 1024
COPY_VIDEO = {
    "mp4": {"h264", "hevc", "av1"},
    "mov": {"h264", "hevc", "prores"},
    "mkv": {"h264", "hevc", "vp8", "vp9", "av1"},
    "webm": {"vp8", "vp9", "av1"},
}
COPY_AUDIO = {
    "mp4": {"aac", "mp3", "opus"},
    "mov": {"aac", "mp3"},
    "mkv": {"aac", "mp3", "vorbis", "opus", "flac"},
    "webm": {"vorbis", "opus"},
}
MAX_FPS = 60.0


def signature(info: dict[str, Any]) -> tuple[Any, ...]:
    """What must match for clips to be joined without re-encoding."""
    v = video_streams(info)[0]
    a = audio_streams(info)
    sound = (a[0].get("codec_name"), a[0].get("sample_rate"), a[0].get("channels")) if a else None
    return (
        v.get("codec_name"),
        v.get("width"),
        v.get("height"),
        v.get("pix_fmt"),
        round(frame_rate(v), 2),
        sound,
    )


def can_copy(infos: list[dict[str, Any]], to: str) -> bool:
    first = signature(infos[0])
    if any(signature(i) != first for i in infos[1:]):
        return False
    codec, *_rest, sound = first
    if codec not in COPY_VIDEO[to]:
        return False
    return sound is None or sound[0] in COPY_AUDIO[to]


def concat_list(paths: list[Path]) -> str:
    """An FFmpeg concat list. Paths are staged inputs (``in/0.mp4``), so
    they need no escaping beyond quotes."""
    return "".join(f"file '{p.resolve().as_posix()}'\n" for p in paths)


def reencode_graph(infos: list[dict[str, Any]], with_sound: bool) -> tuple[str, int, int, float]:
    """Every clip fitted into the first one's frame and rate, then joined.
    Returns the graph and the frame's width, height and rate."""
    v0 = video_streams(infos[0])[0]
    w = max(2, int(v0.get("width") or 640) // 2 * 2)
    h = max(2, int(v0.get("height") or 360) // 2 * 2)
    fps = min(MAX_FPS, frame_rate(v0))
    parts, pads = [], []
    for i, info in enumerate(infos):
        parts.append(
            f"[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,"
            f"pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps:.3f},format=yuv420p[v{i}]"
        )
        pads.append(f"[v{i}]")
        if with_sound:
            if audio_streams(info):
                parts.append(
                    f"[{i}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}]"
                )
            else:
                parts.append(
                    f"anullsrc=r=48000:cl=stereo,atrim=duration={duration_s(info):.3f}[a{i}]"
                )
            pads.append(f"[a{i}]")
    joined = f"{''.join(pads)}concat=n={len(infos)}:v=1:a={1 if with_sound else 0}"
    joined += "[v][a]" if with_sound else "[v]"
    return ";".join([*parts, joined]), w, h, fps


class VideoJoinAdapter(Adapter):
    adapter_id: ClassVar[str] = "video-join"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        durations, pixels = [], []
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            video = video_streams(info)
            if not video:
                return ValidationReport(errors=[issue("inputs", "noVideo")])
            durations.append(duration_s(info))
            pixels.append(int(video[0].get("width") or 0) * int(video[0].get("height") or 0))
        return ValidationReport(metadata={"durations": durations, "pixels": pixels})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        seconds = sum(metadata.get("durations") or [0])
        largest = max(metadata.get("pixels") or [0])
        # x264 keeps a few dozen frames in flight; every input has a decoder.
        return ResourceEstimate(
            ram_bytes=256 * MB + largest * 60 + len(ctx.inputs) * largest * 6,
            disk_bytes=int(seconds * 1.5 * MB) + 16 * MB,
            cost_units=max(1, int(seconds)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = str(params.get("to", "mp4"))
        infos = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            infos.append(probe(ctx, p))
        total = sum(duration_s(i) for i in infos) or 1
        if any(has_extra_streams(i) for i in infos):
            ctx.warn("STREAMS_DROPPED")
        out = ctx.output_path(f"joined.{to}")
        ctx.stage("join", 1, 1)
        if can_copy(infos, to):
            listing = ctx.work_dir / "clips.txt"
            listing.write_text(concat_list(list(ctx.inputs)), "utf-8")
            has_sound = bool(audio_streams(infos[0]))
            args = ["-map", "0:v:0", *(["-map", "0:a:0"] if has_sound else []),
                    "-sn", "-dn", "-map_metadata", "-1", "-c", "copy"]  # fmt: skip
            if to in ("mp4", "mov"):
                args += ["-movflags", "+faststart"]
            args += ["-f", MUXER[to]]
            ffmpeg(
                ctx,
                [listing],
                args,
                out,
                duration=total,
                stage="join",
                input_args=["-f", "concat", "-safe", "0"],
            )
            return [OutputFile(out)]

        ctx.warn("REENCODED")
        with_sound = any(audio_streams(i) for i in infos)
        graph, _, _, _ = reencode_graph(infos, with_sound)
        args = ["-filter_complex", graph, "-map", "[v]"]
        if with_sound:
            args += ["-map", "[a]"]
        args += ["-sn", "-dn", "-map_metadata", "-1", *video_encode_args(to)]
        if not with_sound:
            args += ["-an"]
        args += ["-f", MUXER[to]]
        ffmpeg(ctx, ctx.inputs, args, out, duration=total, stage="join")
        return [OutputFile(out)]


if __name__ == "__main__":
    run_main(VideoJoinAdapter)
