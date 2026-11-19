"""media-clean: remove hidden details from audio and video files: where a
clip was filmed, which phone, the artist, the date, titles and chapters.
Each file keeps its format (ADR 0004). Nothing is re-encoded: the streams
are copied as they are and only the metadata is left behind; data streams
(telemetry) are not copied. A cover picture is copied too, but its bytes are
cleaned first: a photo used as cover art keeps its EXIF, XMP and text chunks."""

from __future__ import annotations

import re
from collections.abc import Collection, Sequence
from pathlib import Path
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
from converter_sdk.limits import MB
from converter_sdk.media import MUXER, duration_s, ffmpeg, probe
from converter_sdk.strip import clean_picture_bytes

SAFE_KEY = re.compile(r"[A-Za-z0-9_.\-]{1,64}")
KEEP_TAGS = {"language"}
ATTACHMENT_TAGS = {"filename", "mimetype"}
"""What a player needs to use an attached file (a font in a Matroska file is
found by these). The name is the attachment's own, not the user's."""


def is_cover(stream: dict[str, Any]) -> bool:
    return bool((stream.get("disposition") or {}).get("attached_pic"))


def is_data(stream: dict[str, Any]) -> bool:
    return stream.get("codec_type") == "data"


COVER_EXT = {"mjpeg": "jpg", "png": "png", "webp": "webp"}
"""The cover codecs whose bytes can be cleaned, and the file name each is written
under. A cover in any other codec (BMP, GIF) is dropped, not copied unchecked."""


def covers_to_clean(info: dict[str, Any]) -> tuple[list[dict[str, Any]], list[int]]:
    """The cover streams that can be cleaned, and the indexes of those that can't."""
    covers = [s for s in info.get("streams") or [] if is_cover(s)]
    cleanable = [s for s in covers if s.get("codec_name") in COVER_EXT]
    return cleanable, [s["index"] for s in covers if s not in cleanable]


def clean_args(
    info: dict[str, Any],
    remove_cover: bool,
    *,
    replaced: Sequence[int] = (),
    dropped: Collection[int] = (),
) -> list[str]:
    """Copy every stream but data streams (GPS tracks and sensor readings travel
    in those), no global or chapter metadata, and every tag of every stream
    cleared (stream tags travel with the streams otherwise), except what an
    attached file needs to be found.

    Cover streams listed in ``replaced`` are not taken from the input: their
    cleaned bytes come as the extra inputs 1, 2, ... in that order, and are
    attached again after the other streams. ``dropped`` are left out."""
    args = ["-map", "0"]
    streams = info.get("streams") or []
    kept = []
    for s in streams:
        if is_data(s) or (remove_cover and is_cover(s)) or s["index"] in {*replaced, *dropped}:
            args += ["-map", f"-0:{s['index']}"]
        else:
            kept.append(s)
    videos = sum(1 for s in kept if s.get("codec_type") == "video")
    for n in range(len(replaced)):
        args += ["-map", f"{n + 1}:0", f"-disposition:v:{videos + n}", "attached_pic"]
    args += ["-c", "copy", "-map_metadata", "-1", "-map_chapters", "-1"]
    for out_index, s in enumerate(kept):
        tags = s.get("tags") or {}
        keep = KEEP_TAGS | (ATTACHMENT_TAGS if s.get("codec_type") == "attachment" else set())
        for key in tags:
            if key.lower() not in keep and SAFE_KEY.fullmatch(str(key)):
                args += [f"-metadata:s:{out_index}", f"{key}="]
        # ``-map_metadata -1`` drops stream tags too, so what is kept is set again.
        for key, value in tags.items():
            if key.lower() in keep and SAFE_KEY.fullmatch(str(key)) and str(value):
                args += [f"-metadata:s:{out_index}", f"{key}={value}"]
    return args


def extract_cover(ctx: JobContext, path: Path, stream: dict[str, Any], tag: str) -> bytes:
    """The picture bytes of one cover stream, copied out without decoding."""
    ext = COVER_EXT[stream["codec_name"]]
    raw = ctx.temp_path(f".{ext}")
    ffmpeg(
        ctx,
        [path],
        [
            "-map",
            f"0:{stream['index']}",
            "-c",
            "copy",
            "-frames:v",
            "1",
            "-f",
            "image2",
            "-update",
            "1",
        ],  # fmt: skip
        raw,
        duration=1,
        stage=tag,
    )
    if raw.stat().st_size > reveal.MAX_MEDIA_BYTES:
        raw.unlink(missing_ok=True)
        raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "pictureSize"})
    data = raw.read_bytes()
    raw.unlink(missing_ok=True)
    return data


def extract_clean_cover(ctx: JobContext, path: Path, stream: dict[str, Any]) -> Path:
    """A file holding one cover picture without its metadata."""
    ctx.check_cancelled()
    data = extract_cover(ctx, path, stream, "cover")
    cleaned = clean_picture_bytes(data, ctx.check_cancelled)
    if cleaned is None:
        raise AdapterError("E_INPUT_CORRUPT")
    out = ctx.temp_path(f".{COVER_EXT[stream['codec_name']]}")
    out.write_bytes(cleaned)
    return out


def cover_details_remain(ctx: JobContext, path: Path, info: dict[str, Any]) -> bool:
    """Whether a cover in the output still carries metadata: read again from the
    output, so it holds whatever the muxer wrote."""
    for s in info.get("streams") or []:
        ctx.check_cancelled()
        if is_cover(s) and s.get("codec_name") in COVER_EXT:
            found = reveal.image_bytes_findings(extract_cover(ctx, path, s, "check"))
            if any(f.kind in reveal.IDENTIFYING_KINDS | {"tags", "comments"} for f in found):
                return True
    return False


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
            remove_cover = bool(params.get("removeCoverArt"))
            covers, unsupported = ([], []) if remove_cover else covers_to_clean(info)
            cover_files = [extract_clean_cover(ctx, path, s) for s in covers]
            args = clean_args(
                info,
                remove_cover,
                replaced=[s["index"] for s in covers],
                dropped=unsupported,
            )
            args += ["-f", MUXER[fmt]]
            out = ctx.output_path(f"{ConversionAdapter.title(params, i)}-clean.{fmt}")
            ffmpeg(
                ctx, [path, *cover_files], args, out, duration=duration_s(info) or 1, stage="clean"
            )
            cleaned = probe(ctx, out)
            after = reveal.media_findings(cleaned)
            if (
                any(f.kind in reveal.IDENTIFYING_KINDS for f in after)
                or any(is_data(s) for s in cleaned.get("streams") or [])
                or cover_details_remain(ctx, out, cleaned)
            ):
                out.unlink(missing_ok=True)
                raise AdapterError("E_INTERNAL", {"reason": "detailsRemain"})
            removed |= bool(before - {(f.kind, f.detail) for f in after}) or bool(unsupported)
            outputs.append(OutputFile(out, source=i))
        if removed:
            ctx.warn("DETAILS_REMOVED")
        return outputs


if __name__ == "__main__":
    run_main(MediaCleanAdapter)
