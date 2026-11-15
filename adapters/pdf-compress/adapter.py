"""pdf-compress: a smaller PDF (ADR 0001, 0004).

Two stages. First, unless the level is lossless, each photo-like picture
(8-bit grey or colour) is resampled so it's no sharper than the chosen DPI
across the largest page, and re-encoded as JPEG; a picture is replaced
only when that makes it clearly smaller. Second, the whole file is
rewritten with compressed object streams and recompressed Flate streams.

Pictures with masks by colour, special colour spaces (CMYK, spot colours,
palettes), 1-bit scans and JBIG2/CCITT/JPEG 2000 data are left alone.
Transparency (/SMask) stays as it was: a mask may have its own size. If
the result isn't smaller, the original is kept (``KEPT_ORIGINAL``)."""

from __future__ import annotations

import io
import shutil
from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import JobContext, OutputFile, run_main
from converter_sdk.convert import ConversionAdapter
from converter_sdk.pdf import PdfAdapter, save_pdf
from pikepdf import Name
from PIL import Image

MB = 1024 * 1024
LEVELS: dict[str, tuple[int, int] | None] = {
    "balanced": (150, 75),
    "strong": (96, 55),
    "lossless": None,
}
"""level → (DPI, JPEG quality), or None to leave pictures alone."""
SKIP_FILTERS = {"/JBIG2Decode", "/CCITTFaxDecode", "/JPXDecode"}
MIN_SIDE = 16
GAIN = 0.9
"""A picture is replaced only if the new data is under 90% of the old."""


def _filters(obj: pikepdf.Stream) -> list[str]:
    f = obj.get(Name.Filter)
    if f is None:
        return []
    return [str(x) for x in f] if isinstance(f, pikepdf.Array) else [str(f)]


def _plain_colour(obj: pikepdf.Stream) -> int | None:
    """Channels (1 or 3) if the colour space is plain grey or RGB, else None."""
    cs = obj.get(Name.ColorSpace)
    if cs == Name.DeviceGray:
        return 1
    if cs == Name.DeviceRGB:
        return 3
    if isinstance(cs, pikepdf.Array) and len(cs) == 2 and cs[0] == Name.ICCBased:
        n = int(cs[1].get(Name.N, 0))
        return n if n in (1, 3) else None
    return None


def candidate(obj: pikepdf.Object) -> bool:
    """Whether a picture is one this tool may re-encode."""
    if not isinstance(obj, pikepdf.Stream) or obj.get(Name.Subtype) != Name.Image:
        return False
    if obj.get(Name.ImageMask, False) or Name.Mask in obj or Name.Decode in obj:
        return False
    if int(obj.get(Name.BitsPerComponent, 0)) != 8 or _plain_colour(obj) is None:
        return False
    return not (set(_filters(obj)) & SKIP_FILTERS)


def recompress(obj: pikepdf.Stream, max_side: int, quality: int) -> tuple[bool, bool]:
    """Re-encode one picture in place: (replaced, downscaled)."""
    try:
        img = pikepdf.PdfImage(obj).as_pil_image()
    except Exception:  # noqa: BLE001 - pikepdf raises many types for odd images
        return False, False
    if img.mode not in ("L", "RGB"):
        return False, False
    downscaled = False
    if max(img.size) > max_side:
        f = max_side / max(img.size)
        img = img.resize(
            (max(MIN_SIDE, round(img.width * f)), max(MIN_SIDE, round(img.height * f))),
            Image.Resampling.LANCZOS,
        )
        downscaled = True
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=quality, optimize=True)
    data = buf.getvalue()
    if len(data) >= len(obj.read_raw_bytes()) * GAIN:
        return False, False
    obj.write(data, filter=Name.DCTDecode)
    obj.Width, obj.Height = img.width, img.height
    obj.BitsPerComponent = 8
    if Name.DecodeParms in obj:
        del obj[Name.DecodeParms]
    return True, downscaled


def page_long_side_in(pdf: pikepdf.Pdf) -> float:
    """The longest page side in the document, in inches."""
    longest = 0.0
    for page in pdf.pages:
        box = page.mediabox
        longest = max(
            longest, abs(float(box[2]) - float(box[0])), abs(float(box[3]) - float(box[1]))
        )
    return max(longest, 72.0) / 72


class PdfCompressAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-compress"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"
    base_ram_bytes: ClassVar[int] = 160 * MB  # pikepdf plus Pillow and one decoded picture
    ram_per_input_byte: ClassVar[float] = 1.0
    disk_factor: ClassVar[float] = 3.0

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        level = LEVELS.get(str(params.get("level", "balanced")), LEVELS["balanced"])
        title = ConversionAdapter.title(params, 0, "document")
        out = ctx.output_path(f"{title}-compressed.pdf")
        source = ctx.inputs[0]
        with ctx.open_pdf(source) as pdf:
            self.warn_shared(ctx, [pdf])
            encrypted = pdf.is_encrypted
            downscaled = False
            if level is not None:
                dpi, quality = level
                max_side = max(MIN_SIDE, round(dpi * page_long_side_in(pdf)))
                seen: set[tuple[int, int]] = set()
                ctx.stage("pictures", 1, 2)
                for n, page in enumerate(pdf.pages, start=1):
                    ctx.check_cancelled()
                    for _, image in page.images.items():
                        key = image.objgen
                        if key in seen or not candidate(image):
                            continue
                        seen.add(key)
                        _, smaller = recompress(image, max_side, quality)
                        downscaled |= smaller
                    ctx.progress("pictures", n, len(pdf.pages))
            ctx.stage("write", 2, 2)
            pdf.remove_unreferenced_resources()
            pages = save_pdf(ctx, pdf, out, recompress_flate=True)
        if downscaled:
            ctx.warn("IMAGE_DOWNSCALED")
        # An encrypted original stays encrypted, so it can't stand in for the
        # rewritten (unlocked) one.
        if not encrypted and out.stat().st_size >= source.stat().st_size:
            shutil.copyfile(source, out)
            ctx.warn("KEPT_ORIGINAL")
        return [OutputFile(out, pages=pages)]


if __name__ == "__main__":
    run_main(PdfCompressAdapter)
