"""images-to-pdf: several images into one PDF, one page each, in the
chosen order (ADR 0003)."""

from __future__ import annotations

import io
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import issue, pdf_bytes_to_output, source_format
from converter_sdk.images import open_image
from converter_sdk.limits import MB
from PIL import ImageOps

SIZES = {"a4": (595.28, 841.89), "letter": (612.0, 792.0)}
MARGINS = {"none": 0.0, "small": 18.0, "large": 54.0}


class ImagesToPdfAdapter(Adapter):
    adapter_id: ClassVar[str] = "images-to-pdf"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        if params.get("pageSize", "image") not in ("image", "a4", "letter"):
            return ValidationReport(errors=[issue("pageSize", "invalid")])
        pixels = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            img = open_image(p)
            pixels.append(img.width * img.height)
            img.close()
        return ValidationReport(metadata={"pixels": pixels})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        pixels = metadata.get("pixels") or [0]
        return ResourceEstimate(
            ram_bytes=160 * MB + max(pixels) * 4 * 4,
            disk_bytes=sum(pixels) * 4,
            cost_units=len(pixels),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        import pymupdf  # noqa: PLC0415

        page_size = params.get("pageSize", "image")
        margin = MARGINS.get(str(params.get("margin", "none")), 0.0)
        ctx.stage("assemble", 1, 2)
        doc = pymupdf.open()
        try:
            for i, path in enumerate(ctx.inputs):
                ctx.check_cancelled()
                img = open_image(path)
                try:
                    img.seek(0)
                    img.load()
                    exif_rotated = ImageOps.exif_transpose(img)
                    dpi = (img.info.get("dpi") or (72, 72))[0] or 72
                    w_pt, h_pt = exif_rotated.width * 72 / dpi, exif_rotated.height * 72 / dpi
                    upright = img.getexif().get(0x0112, 1) in (0, 1)
                    if (
                        source_format(path) == "jpg"
                        and upright
                        and img.mode in ("RGB", "L", "CMYK")
                    ):
                        stream = path.read_bytes()
                    else:
                        buf = io.BytesIO()
                        out = exif_rotated
                        if out.mode not in ("RGB", "RGBA", "L", "LA"):
                            out = out.convert(
                                "RGBA"
                                if "A" in out.getbands() or "transparency" in out.info
                                else "RGB"
                            )
                        out.save(buf, "PNG")
                        stream = buf.getvalue()
                except (OSError, SyntaxError, ValueError, EOFError):
                    raise AdapterError("E_INPUT_CORRUPT") from None
                finally:
                    img.close()
                if page_size == "image":
                    pw, ph = w_pt + 2 * margin, h_pt + 2 * margin
                else:
                    pw, ph = SIZES[page_size]
                    if w_pt > h_pt:  # landscape images get landscape pages
                        pw, ph = ph, pw
                page = doc.new_page(width=pw, height=ph)
                box = pymupdf.Rect(margin, margin, pw - margin, ph - margin)
                page.insert_image(box, stream=stream, keep_proportion=True)
                ctx.progress("assemble", i + 1, len(ctx.inputs))
            data = doc.tobytes(garbage=3, deflate=True)
        finally:
            doc.close()
        ctx.stage("save", 2, 2)
        titles = params.get("fileTitles") or []
        name = f"{titles[0]}.pdf" if len(ctx.inputs) == 1 and titles else "images.pdf"
        out = ctx.output_path(name)
        pages = pdf_bytes_to_output(ctx, data, out)
        return [OutputFile(out, pages=pages)]


if __name__ == "__main__":
    run_main(ImagesToPdfAdapter)
