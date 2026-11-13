"""video-edit: rotate, flip, speed up or slow down, or mute videos, each
keeping its format (ADR 0004). The picture is re-encoded once at high
quality; a rotation flag the camera set is applied first, so the result
plays upright everywhere."""

from __future__ import annotations

from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import ConversionAdapter, issue, source_format
from converter_sdk.media import (
    MUXER,
    atempo,
    audio_streams,
    duration_s,
    ffmpeg,
    has_extra_streams,
    probe,
    video_encode_args,
    video_streams,
)

MB = 1024 * 1024
ROTATE = {90: "transpose=1", 180: "hflip,vflip", 270: "transpose=2"}
FLIP = {"horizontal": "hflip", "vertical": "vflip"}


def video_filter(params: dict[str, Any]) -> str:
    chain = []
    if (r := int(params.get("rotate") or 0)) in ROTATE:
        chain.append(ROTATE[r])
    if (f := params.get("flip", "none")) in FLIP:
        chain.append(FLIP[f])
    if (speed := float(params.get("speed") or 1)) != 1:
        chain.append(f"setpts=PTS/{speed:g}")
    chain.append("scale='trunc(iw/2)*2':'trunc(ih/2)*2',format=yuv420p")
    return ",".join(chain)


def changes(params: dict[str, Any]) -> bool:
    return bool(
        int(params.get("rotate") or 0)
        or params.get("flip", "none") != "none"
        or float(params.get("speed") or 1) != 1
        or params.get("removeAudio")
    )


class VideoEditAdapter(Adapter):
    adapter_id: ClassVar[str] = "video-edit"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        if not changes(params):
            return ValidationReport(errors=[issue("rotate", "nothingToChange")])
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
        return ResourceEstimate(
            ram_bytes=256 * MB + largest * 60,
            disk_bytes=int(seconds * 1.5 * MB) + 16 * MB,
            cost_units=max(1, int(seconds)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        speed = float(params.get("speed") or 1)
        mute = bool(params.get("removeAudio"))
        outputs = []
        dropped = False
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("edit", i + 1, len(ctx.inputs))
            fmt = source_format(path)
            info = probe(ctx, path)
            dropped |= has_extra_streams(info)
            sound = bool(audio_streams(info)) and not mute
            args = ["-map", "0:v:0", *(["-map", "0:a:0"] if sound else []),
                    "-sn", "-dn", "-map_metadata", "0",
                    "-vf", video_filter(params)]  # fmt: skip
            if sound and speed != 1:
                args += ["-af", atempo(speed)]
            args += video_encode_args(fmt)
            if not sound:
                args += ["-an"]
            # Rotation was applied to the pixels; don't let a stale flag turn them again.
            args += ["-metadata:s:v:0", "rotate=0", "-f", MUXER[fmt]]
            title = ConversionAdapter.title(params, i)
            out = ctx.output_path(f"{title}-edited.{fmt}")
            ffmpeg(ctx, [path], args, out, duration=(duration_s(info) / speed) or 1, stage="edit")
            outputs.append(OutputFile(out))
        if dropped:
            ctx.warn("STREAMS_DROPPED")
        return outputs


if __name__ == "__main__":
    run_main(VideoEditAdapter)
