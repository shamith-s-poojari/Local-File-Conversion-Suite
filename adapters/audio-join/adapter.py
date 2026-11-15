"""audio-join: audio files, in the chosen order, as one (ADR 0004).

Every file is decoded and brought to one sample rate (the highest among the
files, at most 48 kHz) and to stereo if any file is stereo, then joined,
with optional silence between files, and encoded once into the chosen
format. Tags aren't carried over: they'd describe only the first file."""

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
from converter_sdk.convert import issue
from converter_sdk.media import (
    MUXER,
    audio_encode_args,
    audio_streams,
    duration_s,
    ffmpeg,
    has_extra_streams,
    probe,
)

MB = 1024 * 1024
MAX_RATE = 48000


def join_graph(count: int, rate: int, layout: str, gap: float) -> str:
    """Each input resampled and padded with ``gap`` seconds of silence (all
    but the last), then concatenated into ``[out]``."""
    parts = []
    for i in range(count):
        pad = f",apad=pad_dur={gap:g}" if gap and i < count - 1 else ""
        parts.append(
            f"[{i}:a:0]aresample={rate},aformat=sample_fmts=fltp:channel_layouts={layout}{pad}[a{i}]"
        )
    inputs = "".join(f"[a{i}]" for i in range(count))
    return ";".join(parts) + f";{inputs}concat=n={count}:v=0:a=1[out]"


class AudioJoinAdapter(Adapter):
    adapter_id: ClassVar[str] = "audio-join"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        durations, rates, channels = [], [], []
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            streams = audio_streams(info)
            if not streams:
                return ValidationReport(errors=[issue("inputs", "noAudio")])
            durations.append(duration_s(info))
            rates.append(int(streams[0].get("sample_rate") or 0))
            channels.append(int(streams[0].get("channels") or 0))
        return ValidationReport(
            metadata={"durations": durations, "rates": rates, "channels": channels}
        )

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        seconds = sum(metadata.get("durations") or [0])
        return ResourceEstimate(
            ram_bytes=192 * MB + len(ctx.inputs) * 8 * MB,
            disk_bytes=int(seconds * 48000 * 2 * 2) + MB,
            cost_units=max(1, int(seconds)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = str(params.get("to", "mp3"))
        gap = float(params.get("gap") or 0)
        rates, layouts, total = [], [], 0.0
        extra = False
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            stream = audio_streams(info)[0]
            rates.append(int(stream.get("sample_rate") or 0))
            layouts.append(int(stream.get("channels") or 0))
            total += duration_s(info)
            extra |= has_extra_streams(info)
        rate = min(MAX_RATE, max(rates) or 44100)
        layout = "stereo" if any(c >= 2 for c in layouts) else "mono"
        if any(r and r != rate for r in rates):
            ctx.warn("AUDIO_RESAMPLED")
        if extra or any(c > 2 for c in layouts):
            ctx.warn("STREAMS_DROPPED")
        total += gap * (len(ctx.inputs) - 1)
        args = [
            "-filter_complex", join_graph(len(ctx.inputs), rate, layout, gap),
            "-map", "[out]", "-map_metadata", "-1", "-vn", "-sn", "-dn",
            *audio_encode_args(to, int(params.get("bitrate") or 192), rate),
            "-f", MUXER[to],
        ]  # fmt: skip
        ctx.stage("join", 1, 1)
        out = ctx.output_path(f"joined.{to}")
        ffmpeg(ctx, ctx.inputs, args, out, duration=total or 1, stage="join")
        return [OutputFile(out)]


if __name__ == "__main__":
    run_main(AudioJoinAdapter)
