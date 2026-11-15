"""audio-convert: between MP3, WAV, FLAC, OGG, Opus, M4A, AAC and AIFF with
FFmpeg (ADR 0003). One output per input, tags kept."""

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
    AUDIO_FORMATS,
    LOSSLESS_AUDIO,
    audio_streams,
    duration_s,
    ffmpeg,
    media_info,
    parse_time,
    probe,
    render_waveform,
)

MB = 1024 * 1024
LOSSY = [f for f in AUDIO_FORMATS if f not in LOSSLESS_AUDIO]
# Decoding keeps every sample, so a lossless target loses nothing more.
# AAC in ADTS moves into M4A without re-encoding.
EDGES: dict[tuple[str, str], Tier] = {
    (src, dst): ("E" if dst in LOSSLESS_AUDIO or (src, dst) == ("aac", "m4a") else "L")
    for src in AUDIO_FORMATS
    for dst in AUDIO_FORMATS
    if src != dst
}

CODEC = {
    "mp3": ["-c:a", "libmp3lame"],
    "ogg": ["-c:a", "libvorbis"],
    "opus": ["-c:a", "libopus"],
    "m4a": ["-c:a", "aac"],
    "aac": ["-c:a", "aac"],
    "flac": ["-c:a", "flac"],
}
MUXER = {
    "mp3": "mp3",
    "ogg": "ogg",
    "opus": "opus",
    "m4a": "ipod",
    "aac": "adts",
    "flac": "flac",
    "wav": "wav",
    "aiff": "aiff",
}
OPUS_RATES = {48000, 24000, 16000, 12000, 8000}


def pcm_codec(fmt: str, bits: int) -> list[str]:
    deep = bits > 16
    if fmt == "wav":
        return ["-c:a", "pcm_s24le" if deep else "pcm_s16le"]
    return ["-c:a", "pcm_s24be" if deep else "pcm_s16be"]


def source_bits(stream: dict[str, Any]) -> int:
    for key in ("bits_per_raw_sample", "bits_per_sample"):
        try:
            value = int(stream.get(key) or 0)
        except (TypeError, ValueError):
            value = 0
        if value:
            return value
    return 16


class AudioConvertAdapter(ConversionAdapter):
    adapter_id: ClassVar[str] = "audio-convert"
    EDGES = EDGES

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        info = probe(ctx, input)
        if not audio_streams(info):
            raise AdapterError("E_INPUT_CORRUPT")
        return InspectReport(
            pages=0,
            encryption="none",
            media=media_info(info, "audio"),
            findings=reveal.media_findings(info),
        )

    def render(self, ctx: JobContext, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        """A waveform: 8 × ``dpi`` pixels wide, a quarter as tall."""
        width = max(240, min(2400, dpi * 8))
        height = max(60, width // 4)
        out.parent.mkdir(parents=True, exist_ok=True)
        render_waveform(ctx, input, out, width, height)
        return RenderedPage(width_px=width, height_px=height)

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        try:
            start = parse_time(params.get("start"))
            end = parse_time(params.get("end"))
        except ValueError:
            return ValidationReport(errors=[issue("start", "invalidTime")])
        durations = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            if not audio_streams(info):
                return ValidationReport(errors=[issue("to", "noAudio")])
            durations.append(duration_s(info))
        if start is not None and end is not None and end <= start:
            return ValidationReport(errors=[issue("end", "beforeStart")])
        if start is not None and durations and start >= min(durations):
            return ValidationReport(errors=[issue("start", "afterEnd")])
        return ValidationReport(metadata={"durations": durations})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        seconds = sum(metadata.get("durations") or [0])
        return ResourceEstimate(
            ram_bytes=160 * MB,
            disk_bytes=int(seconds * 48000 * 2 * 3) + MB,
            cost_units=max(1, int(seconds)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = self.target(params)
        self.use_tiers(ctx, to)
        start = parse_time(params.get("start"))
        end = parse_time(params.get("end"))
        rate = int(params.get("sampleRate") or 0)
        channels = int(params.get("channels") or 0)
        bitrate = int(params.get("bitrate") or 192)
        outputs = []
        total = len(ctx.inputs)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("convert", i + 1, total)
            info = probe(ctx, path)
            streams = audio_streams(info)
            stream = streams[0]
            if len(streams) > 1 or len(info.get("streams", [])) > len(streams):
                ctx.warn("STREAMS_DROPPED")
            src = source_format(path)
            args = ["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "0"]
            copy = (src, to) in (("aac", "m4a"), ("m4a", "aac")) and stream.get(
                "codec_name"
            ) == "aac"
            copy = copy and not (start or end or rate or channels)
            if copy:
                args += ["-c:a", "copy"]
            elif to in ("wav", "aiff"):
                args += pcm_codec(to, source_bits(stream))
            else:
                args += CODEC[to]
                if to in LOSSY:
                    args += ["-b:a", f"{bitrate}k"]
            in_rate = int(stream.get("sample_rate") or 0)
            out_rate = rate
            if to == "opus" and (out_rate or in_rate) not in OPUS_RATES:
                out_rate = 48000
            if out_rate and out_rate != in_rate and not copy:
                args += ["-ar", str(out_rate)]
                ctx.warn("AUDIO_RESAMPLED")
            in_channels = int(stream.get("channels") or 0)
            if channels and not copy and channels < in_channels:
                args += ["-ac", str(channels)]
                if in_channels > 2:
                    ctx.warn("AUDIO_DOWNMIXED")
            elif to in ("mp3",) and in_channels > 2 and not copy:
                # MP3 holds at most two channels.
                args += ["-ac", "2"]
                ctx.warn("AUDIO_DOWNMIXED")
            if to == "flac":
                args += ["-compression_level", "5"]
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


if __name__ == "__main__":
    run_main(AudioConvertAdapter)
