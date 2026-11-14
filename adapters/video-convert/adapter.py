"""video-convert: between MP4, MOV, MKV, WebM and AVI, video to animated GIF
and back, and the soundtrack out of a video, with FFmpeg (ADR 0003)."""

from __future__ import annotations

from pathlib import Path
from typing import Any, ClassVar

from converter_sdk import (
    AdapterError,
    InspectReport,
    JobContext,
    OutputFile,
    RenderedPage,
    ResourceEstimate,
    ValidationReport,
    reveal,
    run_main,
)
from converter_sdk.convert import ConversionAdapter, Tier, issue, source_format
from converter_sdk.media import (
    AUDIO_CODEC,
    AUDIO_FORMATS,
    LOSSLESS_AUDIO,
    MUXER,
    VIDEO_FORMATS,
    audio_streams,
    duration_s,
    ffmpeg,
    media_info,
    parse_time,
    probe,
    render_frame,
    video_streams,
)

MB = 1024 * 1024
REMUX = {"mp4", "mov", "mkv"}
"""Containers that can take each other's H.264/HEVC/AAC streams unchanged."""


def _tier(src: str, dst: str) -> Tier | None:
    if src == dst:
        return None
    if dst in AUDIO_FORMATS:
        return None if src == "gif" else ("E" if dst in LOSSLESS_AUDIO else "L")
    if src in REMUX and dst in REMUX:
        return "H"
    return "L"


EDGES: dict[tuple[str, str], Tier] = {
    (s, d): t
    for s in [*VIDEO_FORMATS, "gif"]
    for d in [*VIDEO_FORMATS, "gif", *AUDIO_FORMATS]
    if (t := _tier(s, d)) is not None
}

CRF = {
    "x264": {"high": 18, "balanced": 23, "small": 28},
    "vp9": {"high": 28, "balanced": 33, "small": 38},
    "mpeg4": {"high": 2, "balanced": 4, "small": 7},
}
COPY_VIDEO = {
    "mp4": {"h264", "hevc", "mpeg4", "av1"},
    "mov": {"h264", "hevc", "mpeg4", "prores", "mjpeg"},
    "mkv": {"h264", "hevc", "mpeg4", "vp8", "vp9", "av1", "mpeg2video", "theora"},
}
COPY_AUDIO = {
    "mp4": {"aac", "mp3", "alac", "ac3", "eac3", "opus"},
    "mov": {"aac", "mp3", "alac", "pcm_s16le", "pcm_s24le"},
    "mkv": {"aac", "mp3", "vorbis", "opus", "flac", "ac3", "eac3", "pcm_s16le"},
}


def _even(max_height: int) -> str:
    """Scale so both sides are even (yuv420p needs it), shrinking only."""
    if max_height:
        return f"scale=-2:'trunc(min(ih,{max_height})/2)*2'"
    return "scale='trunc(iw/2)*2':'trunc(ih/2)*2'"


class VideoConvertAdapter(ConversionAdapter):
    adapter_id: ClassVar[str] = "video-convert"
    EDGES = EDGES

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        info = probe(ctx, input)
        if not video_streams(info):
            raise AdapterError("E_INPUT_CORRUPT")
        return InspectReport(
            pages=0,
            encryption="none",
            media=media_info(info, "video"),
            findings=reveal.media_findings(info),
        )

    def render(self, ctx: JobContext, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        """A poster frame, a tenth of the way in, longest side 8 × ``dpi``."""
        from PIL import Image  # noqa: PLC0415

        info = probe(ctx, input)
        at = min(1.0, duration_s(info) * 0.1)
        out.parent.mkdir(parents=True, exist_ok=True)
        render_frame(ctx, input, out, at, max(120, min(2400, dpi * 8)))
        with Image.open(out) as img:
            return RenderedPage(width_px=img.width, height_px=img.height)

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        try:
            start = parse_time(params.get("start"))
            end = parse_time(params.get("end"))
        except ValueError:
            return ValidationReport(errors=[issue("start", "invalidTime")])
        if start is not None and end is not None and end <= start:
            return ValidationReport(errors=[issue("end", "beforeStart")])
        meta = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            video = video_streams(info)
            if not video:
                raise AdapterError("E_INPUT_CORRUPT")
            if to in AUDIO_FORMATS and not audio_streams(info):
                return ValidationReport(errors=[issue("to", "noAudio")])
            v = video[0]
            meta.append(
                {
                    "seconds": duration_s(info),
                    "pixels": int(v.get("width") or 0) * int(v.get("height") or 0),
                }
            )
        if start is not None and meta and start >= min(m["seconds"] for m in meta):
            return ValidationReport(errors=[issue("start", "afterEnd")])
        return ValidationReport(metadata={"inputs": meta})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        inputs = metadata.get("inputs") or [{"seconds": 0, "pixels": 0}]
        pixels = max(int(m["pixels"]) for m in inputs)
        seconds = sum(float(m["seconds"]) for m in inputs)
        return ResourceEstimate(
            ram_bytes=256 * MB + pixels * 96,
            disk_bytes=int(seconds * 2 * MB) + MB,
            cost_units=max(1, int(seconds * max(1, pixels // 100_000))),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = self.target(params)
        self.use_tiers(ctx, to)
        start = parse_time(params.get("start"))
        end = parse_time(params.get("end"))
        outputs = []
        total = len(ctx.inputs)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("convert", i + 1, total)
            info = probe(ctx, path)
            if to in AUDIO_FORMATS:
                args = self._audio_args(ctx, info, to, params)
            elif to == "gif":
                args = self._gif_args(params)
            else:
                args = self._video_args(
                    ctx,
                    info,
                    source_format(path),
                    to,
                    params,
                    trimmed=start is not None or end is not None,
                )
            args += ["-f", MUXER[to]]
            input_args: list[str] = []
            length = duration_s(info)
            if start is not None:
                input_args += ["-ss", f"{start:.3f}"]
                length = max(0.0, length - start)
            if end is not None:
                input_args += ["-to", f"{end:.3f}"]
                length = min(length, end - (start or 0))
            out = ctx.output_path(self.output_name(params, i, to))
            ffmpeg(
                ctx, [path], args, out, duration=length or 1, stage="convert", input_args=input_args
            )
            outputs.append(OutputFile(out))
        return outputs

    # ------------------------------------------------------------- parts

    def _audio_args(
        self, ctx: JobContext, info: dict[str, Any], to: str, params: dict[str, Any]
    ) -> list[str]:
        args = ["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "0", *AUDIO_CODEC[to]]
        stream = audio_streams(info)[0]
        if to in ("m4a", "aac") and stream.get("codec_name") == "aac":
            args[-2:] = ["-c:a", "copy"]
        elif to not in LOSSLESS_AUDIO:
            args += ["-b:a", f"{int(params.get('bitrate') or 192)}k"]
        if to == "opus" and int(stream.get("sample_rate") or 0) not in (
            48000,
            24000,
            16000,
            12000,
            8000,
        ):
            args += ["-ar", "48000"]
        if to == "mp3" and int(stream.get("channels") or 0) > 2:
            args += ["-ac", "2"]
            ctx.warn("AUDIO_DOWNMIXED")
        return args

    def _gif_args(self, params: dict[str, Any]) -> list[str]:
        fps = int(params.get("fps") or 12)
        width = int(params.get("gifWidth") or 480)
        graph = (
            f"[0:v:0]fps={fps},scale='min({width},iw)':-1:flags=lanczos,split[a][b];"
            "[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4"
        )
        return ["-filter_complex", graph, "-an", "-loop", "0"]

    def _video_args(
        self,
        ctx: JobContext,
        info: dict[str, Any],
        src: str,
        to: str,
        params: dict[str, Any],
        *,
        trimmed: bool,
    ) -> list[str]:
        quality = str(params.get("quality") or "original")
        max_height = int(params.get("maxHeight") or 0)
        mute = bool(params.get("removeAudio"))
        v = video_streams(info)[0]
        audio = audio_streams(info)
        others = [s for s in info.get("streams", []) if s.get("codec_type") in ("subtitle", "data")]
        if others or len(audio) > 1:
            ctx.warn("STREAMS_DROPPED")
        height = int(v.get("height") or 0)
        resize = bool(max_height and height > max_height)
        args = ["-map", "0:v:0"]
        if audio and not mute:
            args += ["-map", "0:a:0"]
        args += ["-sn", "-dn", "-map_metadata", "0"]

        copy_video = (
            quality == "original"
            and not resize
            and not trimmed
            and to in COPY_VIDEO
            and v.get("codec_name") in COPY_VIDEO[to]
        )
        if copy_video:
            args += ["-c:v", "copy"]
        elif to == "webm":
            args += ["-c:v", "libvpx-vp9", "-crf", str(CRF["vp9"][_level(quality)]), "-b:v", "0",
                     "-row-mt", "1", "-deadline", "good", "-cpu-used", "4",
                     "-pix_fmt", "yuv420p", "-vf", _even(max_height)]  # fmt: skip
        elif to == "avi":
            args += ["-c:v", "mpeg4", "-vtag", "xvid",
                     "-q:v", str(CRF["mpeg4"][_level(quality)]),
                     "-pix_fmt", "yuv420p", "-vf", _even(max_height)]  # fmt: skip
        else:
            args += ["-c:v", "libx264", "-preset", "medium",
                     "-crf", str(CRF["x264"][_level(quality)]),
                     "-pix_fmt", "yuv420p", "-vf", _even(max_height)]  # fmt: skip
        if audio and not mute:
            codec = audio[0].get("codec_name")
            if copy_video and to in COPY_AUDIO and codec in COPY_AUDIO[to]:
                args += ["-c:a", "copy"]
            elif to == "webm":
                args += ["-c:a", "libopus", "-b:a", "128k", "-ar", "48000"]
            elif to == "avi":
                args += ["-c:a", "libmp3lame", "-b:a", "192k"]
            else:
                args += ["-c:a", "aac", "-b:a", "160k"]
        if to in ("mp4", "mov"):
            args += ["-movflags", "+faststart"]
        return args


def _level(quality: str) -> str:
    return quality if quality in ("high", "balanced", "small") else "high"


if __name__ == "__main__":
    run_main(VideoConvertAdapter)
