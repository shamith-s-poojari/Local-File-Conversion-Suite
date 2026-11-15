"""image-edit: rotate, flip, resize or turn images black and white. Each
file keeps its format (ADR 0004); animations keep every frame."""

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
from converter_sdk.convert import ConversionAdapter, issue, source_format
from converter_sdk.images import Frames, encode, pixels, read_frames
from PIL import Image, ImageOps

MB = 1024 * 1024
TRANSPOSE = {
    90: Image.Transpose.ROTATE_270,  # Pillow's rotations are anticlockwise
    180: Image.Transpose.ROTATE_180,
    270: Image.Transpose.ROTATE_90,
}
FLIP = {
    "horizontal": Image.Transpose.FLIP_LEFT_RIGHT,
    "vertical": Image.Transpose.FLIP_TOP_BOTTOM,
}
MAX_OUTPUT_PIXELS = 200_000_000


def new_size(size: tuple[int, int], params: dict[str, Any]) -> tuple[int, int]:
    """The size after resizing (rotation already applied to ``size``)."""
    w, h = size
    mode = params.get("resize", "none")
    if mode == "percent":
        f = int(params.get("percent", 100)) / 100
        return max(1, round(w * f)), max(1, round(h * f))
    if mode == "fit":
        bw, bh = int(params.get("width", w)), int(params.get("height", h))
        f = min(bw / w, bh / h, 1.0)
        return max(1, round(w * f)), max(1, round(h * f))
    return w, h


def edit(img: Image.Image, params: dict[str, Any]) -> Image.Image:
    if (r := int(params.get("rotate", 0))) in TRANSPOSE:
        img = img.transpose(TRANSPOSE[r])
    if (f := params.get("flip", "none")) in FLIP:
        img = img.transpose(FLIP[f])
    size = new_size(img.size, params)
    if size != img.size:
        if img.mode == "P":
            img = img.convert("RGBA")
        img = img.resize(size, Image.Resampling.LANCZOS)
    if params.get("grayscale"):
        img = (
            ImageOps.grayscale(img)
            if img.mode in ("RGB", "L", "P", "CMYK") and not _alpha(img)
            else _gray_alpha(img)
        )
    return img


def _alpha(img: Image.Image) -> bool:
    return "A" in img.getbands() or "transparency" in img.info


def _gray_alpha(img: Image.Image) -> Image.Image:
    rgba = img.convert("RGBA")
    gray = ImageOps.grayscale(rgba.convert("RGB"))
    return Image.merge("LA", (gray, rgba.getchannel("A")))


class ImageEditAdapter(Adapter):
    adapter_id: ClassVar[str] = "image-edit"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        sizes = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            n = pixels(p)
            sizes.append(n)
            if params.get("resize") == "percent":
                f = int(params.get("percent", 100)) / 100
                if n * f * f > MAX_OUTPUT_PIXELS:
                    return ValidationReport(errors=[issue("percent", "tooLarge")])
        return ValidationReport(metadata={"pixels": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        sizes = metadata.get("pixels") or [0]
        f = 1.0
        if params.get("resize") == "percent":
            f = max(1.0, (int(params.get("percent", 100)) / 100) ** 2)
        return ResourceEstimate(
            ram_bytes=round(128 * MB + max(sizes) * 4 * 4 * f),
            disk_bytes=round(sum(sizes) * 4 * f),
            cost_units=max(1, len(sizes)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        outputs = []
        total = len(ctx.inputs)
        ctx.stage("edit", 1, 1)
        downscaled = False
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            fmt = source_format(path)
            f = read_frames(ctx, path)
            before = f.first.size
            frames = []
            for frame in f.frames:
                ctx.check_cancelled()
                frames.append(edit(frame, params))
            after = frames[0].size
            if params.get("resize") == "fit" and max(after) < max(before):
                downscaled = True
            data = encode(
                Frames(frames, f.durations, f.loop, f.exif, f.icc),
                fmt,
                quality=92,
                keep_exif=not params.get("stripMetadata", False),
                lossless_webp=fmt == "webp" and _lossless_webp(path),
            )
            title = ConversionAdapter.title(params, i)
            out = ctx.output_path(f"{title}-edited.{fmt}")
            out.write_bytes(data)
            outputs.append(OutputFile(out, source=i))
            ctx.progress("edit", i + 1, total)
        if downscaled:
            ctx.warn("IMAGE_DOWNSCALED")
        return outputs


def _lossless_webp(path: Path) -> bool:
    """WebP files that were lossless stay lossless (VP8L chunk)."""
    with path.open("rb") as fh:
        head = fh.read(64)
    return b"VP8L" in head


if __name__ == "__main__":
    run_main(ImageEditAdapter)
