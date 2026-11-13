"""image-compress: each image under a target file size, in its own format
(ADR 0004). Lossy formats lower their quality first; PNG drops to 256
colours; then the picture shrinks. A file already under the target, or
one that can't get smaller, is kept as it was."""

from __future__ import annotations

import math
import shutil
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
from converter_sdk.convert import ConversionAdapter, source_format
from converter_sdk.images import Frames, encode, pixels, read_frames, to_palette
from PIL import Image

MB = 1024 * 1024
LOSSY = ("jpg", "webp", "avif")
QUALITIES = [85, 75, 65, 55, 45, 35]
"""Tried from the top; the first that fits wins."""
MIN_SIDE = 64
"""Below this the picture is useless, so shrinking stops."""
MAX_SHRINKS = 8


def _smallest(
    img: Image.Image, icc: bytes | None, fmt: str, target: int, ctx: JobContext
) -> tuple[bytes, bool]:
    """The best encoding of ``img`` at this size: (bytes, fits the target).
    The colour profile is kept (it's small, and colours look wrong without
    it); camera details aren't."""
    if fmt in LOSSY:
        best = b""
        for q in QUALITIES:
            ctx.check_cancelled()
            data = encode(Frames([img], icc=icc), fmt, quality=q, keep_exif=False)
            if len(data) <= target:
                return data, True
            best = data
        return best, False
    full = encode(Frames([img], icc=icc), "png", keep_exif=False)
    if len(full) <= target:
        return full, True
    ctx.check_cancelled()
    data = encode(Frames([to_palette(img)], icc=icc), "png", keep_exif=False)
    return data, len(data) <= target


class ImageCompressAdapter(Adapter):
    adapter_id: ClassVar[str] = "image-compress"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        sizes = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            sizes.append(pixels(p))
        return ValidationReport(metadata={"pixels": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        sizes = metadata.get("pixels") or [0]
        return ResourceEstimate(
            ram_bytes=128 * MB + max(sizes) * 4 * 5,
            disk_bytes=sum(sizes) * 2,
            cost_units=max(1, len(sizes) * 4),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        target = int(params.get("targetKb", 500)) * 1000
        outputs = []
        total = len(ctx.inputs)
        notes: set[str] = set()
        ctx.stage("compress", 1, 1)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            fmt = source_format(path)
            title = ConversionAdapter.title(params, i)
            out = ctx.output_path(f"{title}-compressed.{fmt}")
            original = path.stat().st_size
            if original <= target:
                read_frames(ctx, path, first_only=True)  # a damaged file fails here
                shutil.copyfile(path, out)
                notes.add("KEPT_ORIGINAL")
            else:
                data = self._compress(ctx, path, fmt, target, notes)
                if len(data) >= original:
                    shutil.copyfile(path, out)
                    notes.add("KEPT_ORIGINAL")
                else:
                    out.write_bytes(data)
            outputs.append(OutputFile(out))
            ctx.progress("compress", i + 1, total)
        for code in sorted(notes):
            ctx.warn(code)
        return outputs

    def _compress(
        self, ctx: JobContext, path: Path, fmt: str, target: int, notes: set[str]
    ) -> bytes:
        f = read_frames(ctx, path, first_only=True)
        if f.exif:
            notes.add("METADATA_DROPPED")
        img = f.first
        with Image.open(path) as probe:
            if int(getattr(probe, "n_frames", 1)) > 1:
                notes.add("ANIMATION_FLATTENED")
        data, fits = _smallest(img, f.icc, fmt, target, ctx)
        shrinks = 0
        while not fits and min(img.size) > MIN_SIDE and shrinks < MAX_SHRINKS:
            ctx.check_cancelled()
            # Size scales roughly with pixel count; aim a little under.
            factor = max(0.5, min(0.9, math.sqrt(target / len(data)) * 0.95))
            w, h = img.size
            img = img.resize(
                (max(MIN_SIDE, round(w * factor)), max(MIN_SIDE, round(h * factor))),
                Image.Resampling.LANCZOS,
            )
            data, fits = _smallest(img, f.icc, fmt, target, ctx)
            shrinks += 1
            notes.add("IMAGE_DOWNSCALED")
        if fmt == "png" and data[25:26] == b"\x03":  # IHDR colour type 3: palette
            notes.add("COLORS_REDUCED")
        if not fits:
            notes.add("TARGET_NOT_REACHED")
        return data


if __name__ == "__main__":
    run_main(ImageCompressAdapter)
