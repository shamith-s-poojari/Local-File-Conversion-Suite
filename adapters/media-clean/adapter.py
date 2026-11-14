"""media-clean: remove hidden details from audio and video files: where a
clip was filmed, which phone, the artist, the date, titles and chapters.
Each file keeps its format (ADR 0004). Nothing is re-encoded: the streams
are copied as they are and only the metadata is left behind."""

from __future__ import annotations

import re
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    reveal,
    run_main,
)
from converter_sdk.convert import ConversionAdapter, source_format
from converter_sdk.errors import AdapterError
from converter_sdk.media import MUXER, duration_s, ffmpeg, probe

MB = 1024 * 1024
SAFE_KEY = re.compile(r"[A-Za-z0-9_.\-]{1,64}")
KEEP_TAGS = {"language"}


def is_cover(stream: dict[str, Any]) -> bool:
    return bool((stream.get("disposition") or {}).get("attached_pic"))


def clean_args(info: dict[str, Any], remove_cover: bool) -> list[str]:
    """Copy every stream, no global or chapter metadata, and every tag of
    every stream cleared (stream tags travel with the streams otherwise)."""
    args = ["-map", "0"]
    streams = info.get("streams") or []
    kept = []
    for s in streams:
        if remove_cover and is_cover(s):
            args += ["-map", f"-0:{s['index']}"]
        else:
            kept.append(s)
    args += ["-c", "copy", "-map_metadata", "-1", "-map_chapters", "-1"]
    for out_index, s in enumerate(kept):
        for key in s.get("tags") or {}:
            if key.lower() not in KEEP_TAGS and SAFE_KEY.fullmatch(str(key)):
                args += [f"-metadata:s:{out_index}", f"{key}="]
    return args


class MediaCleanAdapter(Adapter):
    adapter_id: ClassVar[str] = "media-clean"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        durations, sizes = [], []
        for p in ctx.inputs:
            ctx.check_cancelled()
            durations.append(duration_s(probe(ctx, p)))
            sizes.append(p.stat().st_size)
        return ValidationReport(metadata={"durations": durations, "bytes": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        sizes = metadata.get("bytes") or [0]
        return ResourceEstimate(
            ram_bytes=256 * MB,
            disk_bytes=sum(sizes) + 16 * MB,
            cost_units=max(1, int(sum(metadata.get("durations") or [0]))),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        outputs: list[OutputFile] = []
        removed = False
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("clean", i + 1, len(ctx.inputs))
            fmt = source_format(path)
            info = probe(ctx, path)
            before = {(f.kind, f.detail) for f in reveal.media_findings(info)}
            args = clean_args(info, bool(params.get("removeCoverArt")))
            args += ["-f", MUXER[fmt]]
            out = ctx.output_path(f"{ConversionAdapter.title(params, i)}-clean.{fmt}")
            ffmpeg(ctx, [path], args, out, duration=duration_s(info) or 1, stage="clean")
            after = reveal.media_findings(probe(ctx, out))
            if any(f.kind in reveal.IDENTIFYING_KINDS for f in after):
                out.unlink(missing_ok=True)
                raise AdapterError("E_INTERNAL", {"reason": "detailsRemain"})
            removed |= bool(before - {(f.kind, f.detail) for f in after})
            outputs.append(OutputFile(out, source=i))
        if removed:
            ctx.warn("DETAILS_REMOVED")
        return outputs


if __name__ == "__main__":
    run_main(MediaCleanAdapter)
