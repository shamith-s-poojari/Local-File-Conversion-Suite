"""images-to-gif: pictures, in the chosen order, as one animated GIF
(ADR 0004). Every frame has the first picture's proportions at the chosen
width; others are fitted inside and centred on white."""

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
from converter_sdk.images import WHITE, flatten, has_alpha, pixels, read_frames
from PIL import Image

MB = 1024 * 1024


def fit_on_canvas(img: Image.Image, size: tuple[int, int]) -> Image.Image:
    """``img`` scaled to fit inside ``size``, centred on white."""
    img = flatten(img) if has_alpha(img) else img.convert("RGB")
    w, h = size
    f = min(w / img.width, h / img.height)
    scaled = img.resize(
        (max(1, round(img.width * f)), max(1, round(img.height * f))), Image.Resampling.LANCZOS
    )
    canvas = Image.new("RGB", size, WHITE)
    canvas.paste(scaled, ((w - scaled.width) // 2, (h - scaled.height) // 2))
    return canvas


class ImagesToGifAdapter(Adapter):
    adapter_id: ClassVar[str] = "images-to-gif"
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
        width = int(params.get("width", 480))
        frame = width * width * 2  # up to a 1:2 portrait canvas, palette bytes
        return ResourceEstimate(
            ram_bytes=160 * MB + max(sizes) * 4 * 3 + len(sizes) * frame * 4,
            disk_bytes=len(sizes) * frame,
            cost_units=len(sizes),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        width = int(params.get("width", 480))
        frames: list[Image.Image] = []
        size: tuple[int, int] | None = None
        flattened = False
        ctx.stage("frames", 1, 2)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            f = read_frames(ctx, path, first_only=True)
            with Image.open(path) as probe:
                flattened |= int(getattr(probe, "n_frames", 1)) > 1
            img = f.first
            if size is None:
                # Height from the first picture's shape, at most twice the width.
                size = (width, max(16, min(width * 2, round(width * img.height / img.width))))
            frames.append(
                fit_on_canvas(img, size).quantize(colors=256, dither=Image.Dither.FLOYDSTEINBERG)
            )
            ctx.progress("frames", i + 1, len(ctx.inputs))
        if flattened:
            ctx.warn("ANIMATION_FLATTENED")
        ctx.warn("COLORS_REDUCED")
        ctx.stage("save", 2, 2)
        ctx.check_cancelled()
        out = ctx.output_path("animation.gif")
        save: dict[str, Any] = {
            "save_all": True,
            "append_images": frames[1:],
            "duration": int(params.get("frameMs", 500)),
            "optimize": False,
            "disposal": 1,
        }
        if params.get("loop", True):
            save["loop"] = 0
        frames[0].save(out, "GIF", **save)
        return [OutputFile(out)]


if __name__ == "__main__":
    run_main(ImagesToGifAdapter)
