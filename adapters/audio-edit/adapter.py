"""audio-edit: volume, loudness, fades and speed, each file keeping its
format (ADR 0004). Lossy files are re-encoded at about their own bitrate;
the sample rate, channels and tags stay as they were."""

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
    audio_encode_args,
    audio_streams,
    duration_s,
    ffmpeg,
    has_extra_streams,
    probe,
)

MB = 1024 * 1024
BITRATES = [96, 128, 160, 192, 256, 320]


def changes(params: dict[str, Any]) -> bool:
    return bool(
        params.get("normalize")
        or int(params.get("volume") or 0)
        or float(params.get("fadeIn") or 0)
        or float(params.get("fadeOut") or 0)
        or float(params.get("speed") or 1) != 1
    )


def filters(params: dict[str, Any], seconds: float) -> str:
    """The filter chain, for a file ``seconds`` long before the speed change."""
    chain = []
    speed = float(params.get("speed") or 1)
    if speed != 1:
        chain.append(atempo(speed))
    length = seconds / speed
    if params.get("normalize"):
        chain.append("loudnorm=I=-16:TP=-1.5:LRA=11")
    if volume := int(params.get("volume") or 0):
        chain.append(f"volume={volume}dB")
    if fade_in := float(params.get("fadeIn") or 0):
        chain.append(f"afade=t=in:st=0:d={min(fade_in, length):g}")
    if fade_out := float(params.get("fadeOut") or 0):
        d = min(fade_out, length)
        chain.append(f"afade=t=out:st={max(0.0, length - d):.3f}:d={d:g}")
    return ",".join(chain)


def nearest_bitrate(stream: dict[str, Any], info: dict[str, Any]) -> int:
    """The source's bitrate rounded to a standard step (192 if unknown)."""
    raw = stream.get("bit_rate") or info.get("format", {}).get("bit_rate")
    try:
        kbps = int(raw) / 1000
    except (TypeError, ValueError):
        return 192
    return min(BITRATES, key=lambda b: abs(b - kbps))


class AudioEditAdapter(Adapter):
    adapter_id: ClassVar[str] = "audio-edit"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        if not changes(params):
            return ValidationReport(errors=[issue("volume", "nothingToChange")])
        durations = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            if not audio_streams(info):
                return ValidationReport(errors=[issue("inputs", "noAudio")])
            durations.append(duration_s(info))
        return ValidationReport(metadata={"durations": durations})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        seconds = sum(metadata.get("durations") or [0])
        return ResourceEstimate(
            ram_bytes=192 * MB,
            disk_bytes=int(seconds * 48000 * 2 * 3) + MB,
            cost_units=max(1, int(seconds)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        outputs = []
        dropped = False
        speed = float(params.get("speed") or 1)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("edit", i + 1, len(ctx.inputs))
            fmt = source_format(path)
            info = probe(ctx, path)
            stream = audio_streams(info)[0]
            dropped |= has_extra_streams(info)
            seconds = duration_s(info)
            rate = int(stream.get("sample_rate") or 0)
            args = [
                "-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "0",
                "-af", filters(params, seconds),
                *audio_encode_args(fmt, nearest_bitrate(stream, info), rate),
                "-f", MUXER[fmt],
            ]  # fmt: skip
            title = ConversionAdapter.title(params, i)
            out = ctx.output_path(f"{title}-edited.{fmt}")
            ffmpeg(ctx, [path], args, out, duration=(seconds / speed) or 1, stage="edit")
            outputs.append(OutputFile(out))
        if dropped:
            ctx.warn("STREAMS_DROPPED")
        return outputs


if __name__ == "__main__":
    run_main(AudioEditAdapter)
