"""image-convert: between raster formats, SVG and HEIC to raster, and any
image to PDF (ADR 0003). One output per input."""

from __future__ import annotations

import io
import warnings
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
    run_main,
)
from converter_sdk.convert import ConversionAdapter, Tier, pdf_bytes_to_output, source_format
from converter_sdk.protocol import MediaInfo
from PIL import Image, ImageOps, ImageSequence, UnidentifiedImageError

MB = 1024 * 1024
MAX_PIXELS = 200_000_000
"""Larger images fail with E_LIMIT_EXCEEDED (decompression-bomb guard)."""
SVG_SCALE = 2.0
"""SVG has no pixel size of its own; it's rendered at twice its declared size."""

SOURCES = ["png", "jpg", "webp", "heic", "gif", "bmp", "tiff", "avif", "ico", "svg"]
TARGETS = ["png", "jpg", "webp", "gif", "bmp", "tiff", "avif", "ico", "pdf"]
EXACT_DECODE = {"png", "jpg", "webp", "gif", "bmp", "tiff", "ico"}
"""Sources whose decoded pixels are exactly what the file holds."""
LOSSY_CODED = {"jpg", "heic", "avif"}
"""Photos already encoded lossily: WebP output stays lossy for these."""

# The blueprint matrix fixes these cells; the rest follow the same reasoning.
BLUEPRINT: dict[tuple[str, str], Tier] = {
    ("png", "jpg"): "L", ("png", "webp"): "E", ("png", "pdf"): "E",
    ("jpg", "png"): "E", ("jpg", "webp"): "L", ("jpg", "pdf"): "E",
    ("webp", "png"): "E", ("webp", "jpg"): "L", ("webp", "pdf"): "E",
    ("svg", "png"): "H", ("svg", "jpg"): "L", ("svg", "webp"): "H", ("svg", "pdf"): "H",
    ("heic", "png"): "H", ("heic", "jpg"): "L", ("heic", "webp"): "L", ("heic", "pdf"): "H",
}  # fmt: skip
IN_BLUEPRINT = {"png", "jpg", "webp", "svg", "heic", "pdf"}


def _tier(src: str, dst: str) -> Tier | None:
    if src == dst:
        return None
    if src in IN_BLUEPRINT and dst in IN_BLUEPRINT:
        return BLUEPRINT.get((src, dst))
    if dst in ("jpg", "avif", "gif", "ico"):
        return "L"
    if dst == "webp":
        return "L" if src in LOSSY_CODED else ("E" if src in EXACT_DECODE else "H")
    # png, tiff, bmp and pdf keep every decoded pixel.
    return "E" if src in EXACT_DECODE else "H"


EDGES: dict[tuple[str, str], Tier] = {
    (s, t): tier for s in SOURCES for t in TARGETS if (tier := _tier(s, t)) is not None
}

PIL_FORMAT = {
    "png": "PNG", "jpg": "JPEG", "webp": "WEBP", "gif": "GIF", "bmp": "BMP",
    "tiff": "TIFF", "avif": "AVIF", "ico": "ICO",
}  # fmt: skip
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def _register_heif() -> None:
    try:
        import pillow_heif  # noqa: PLC0415 - optional engine

        pillow_heif.register_heif_opener()
    except ImportError:
        raise AdapterError("E_INSTALL_CORRUPT", {"missing": "pillow-heif"}) from None


def _hex(color: str) -> tuple[int, int, int]:
    c = color.lstrip("#")
    return int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16)


class Decoded:
    """An input decoded to frames, with what's needed to write it back out."""

    def __init__(
        self,
        frames: list[Image.Image],
        durations: list[int],
        loop: int,
        info: dict[str, Any],
        total: int = 1,
    ):
        self.frames = frames
        self.total_frames = max(total, len(frames))
        self.durations = durations
        self.loop = loop
        self.exif: bytes | None = info.get("exif")
        self.icc: bytes | None = info.get("icc_profile")
        self.dpi: tuple[float, float] | None = info.get("dpi")

    @property
    def first(self) -> Image.Image:
        return self.frames[0]

    @property
    def animated(self) -> bool:
        return self.total_frames > 1


def _open(path: Path) -> Image.Image:
    fmt = source_format(path)
    if fmt == "heic":
        _register_heif()
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("error", Image.DecompressionBombWarning)
            img = Image.open(path)
            if img.width * img.height > MAX_PIXELS:
                raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "pixels"})
            return img
    except AdapterError:
        raise
    except (Image.DecompressionBombError, Image.DecompressionBombWarning):
        raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "pixels"}) from None
    except (UnidentifiedImageError, OSError, SyntaxError, ValueError):
        raise AdapterError("E_INPUT_CORRUPT") from None


def _svg_document(path: Path):
    import pymupdf  # noqa: PLC0415 - engine imported on demand

    try:
        doc = pymupdf.open(path, filetype="svg")
        if doc.page_count < 1:
            raise AdapterError("E_INPUT_CORRUPT")
        return doc
    except AdapterError:
        raise
    except Exception:
        raise AdapterError("E_INPUT_CORRUPT") from None


def _svg_size(path: Path) -> tuple[int, int]:
    doc = _svg_document(path)
    try:
        r = doc[0].rect
        return max(1, round(r.width * SVG_SCALE)), max(1, round(r.height * SVG_SCALE))
    finally:
        doc.close()


def _render_svg(path: Path, max_side: int = 0) -> Image.Image:
    import pymupdf  # noqa: PLC0415

    doc = _svg_document(path)
    try:
        page = doc[0]
        w, h = page.rect.width * SVG_SCALE, page.rect.height * SVG_SCALE
        if w * h > MAX_PIXELS:
            raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "pixels"})
        scale = SVG_SCALE
        if max_side and max(w, h) > max_side:
            scale *= max_side / max(w, h)
        pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale), alpha=True)
        return Image.frombytes("RGBA", (pix.width, pix.height), pix.samples)
    finally:
        doc.close()


def decode(ctx: JobContext, path: Path, *, max_side: int = 0, first_only: bool = False) -> Decoded:
    if source_format(path) == "svg":
        return Decoded([_render_svg(path, max_side)], [0], 0, {})
    img = _open(path)
    try:
        info = dict(img.info)
        frames: list[Image.Image] = []
        durations: list[int] = []
        total = int(getattr(img, "n_frames", 1))
        n = 1 if first_only else total
        for i, frame in enumerate(ImageSequence.Iterator(img)):
            if i >= n:
                break
            ctx.check_cancelled()
            frame.load()
            copy = frame.copy()
            if i == 0:
                copy = ImageOps.exif_transpose(copy)
            frames.append(copy)
            durations.append(int(frame.info.get("duration", info.get("duration", 100)) or 100))
        if not frames:
            raise AdapterError("E_INPUT_CORRUPT")
        if source_format(path) == "ico":
            # The largest icon in the file, not the first.
            frames = [max(frames, key=lambda f: f.width * f.height)]
        loop = int(info.get("loop", 0) or 0)
        if source_format(path) == "ico":
            total = 1
        return Decoded(frames, durations, loop, info, total)
    except (OSError, SyntaxError, ValueError, EOFError):
        raise AdapterError("E_INPUT_CORRUPT") from None
    finally:
        img.close()


def _fit(img: Image.Image, max_side: int) -> Image.Image:
    if max_side and max(img.size) > max_side:
        copy = img.copy()
        copy.thumbnail((max_side, max_side), Image.Resampling.LANCZOS)
        return copy
    return img


def _has_alpha(img: Image.Image) -> bool:
    if img.mode in ("RGBA", "LA", "PA"):
        return img.getextrema()[-1][0] < 255
    return img.mode == "P" and "transparency" in img.info


def _flatten(img: Image.Image, background: tuple[int, int, int]) -> Image.Image:
    rgba = img.convert("RGBA")
    base = Image.new("RGB", rgba.size, background)
    base.paste(rgba, mask=rgba.getchannel("A"))
    return base


def _rgb_or_rgba(img: Image.Image) -> Image.Image:
    if img.mode in ("RGB", "RGBA", "L", "LA"):
        return img
    if img.mode in ("I;16", "I;16B", "I;16L", "I"):
        return img.convert("I;16") if img.mode != "I" else img
    return img.convert("RGBA" if _has_alpha(img) else "RGB")


def _clean_exif(exif: bytes | None, strip: bool) -> bytes | None:
    """EXIF without the orientation tag (pixels are already upright), or none."""
    if strip or not exif:
        return None
    try:
        e = Image.Exif()
        e.load(exif)
        e.pop(0x0112, None)
        return e.tobytes()
    except Exception:
        return None


class ImageConvertAdapter(ConversionAdapter):
    adapter_id: ClassVar[str] = "image-convert"
    EDGES = EDGES

    # ------------------------------------------------------------- inspect

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        if source_format(input) == "svg":
            w, h = _svg_size(input)
            return InspectReport(
                pages=1,
                encryption="none",
                media=MediaInfo(kind="image", width=w, height=h, frames=1),
            )
        img = _open(input)
        try:
            frames = int(getattr(img, "n_frames", 1))
            return InspectReport(
                pages=1,
                encryption="none",
                media=MediaInfo(kind="image", width=img.width, height=img.height, frames=frames),
            )
        finally:
            img.close()

    def render(self, ctx: JobContext, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        """A preview: the first frame, longest side ``dpi * 8`` pixels."""
        side = max(64, min(4096, dpi * 8))
        img = decode(ctx, input, max_side=side, first_only=True).first
        img = _fit(img, side)
        img = img.convert("RGBA" if _has_alpha(img) else "RGB")
        img.save(out, "PNG")
        return RenderedPage(width_px=img.width, height_px=img.height)

    # ---------------------------------------------------------- validation

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        pixels = []
        for p in ctx.inputs:
            ctx.check_cancelled()
            if source_format(p) == "svg":
                w, h = _svg_size(p)
            else:
                img = _open(p)
                w, h = img.size
                frames = int(getattr(img, "n_frames", 1))
                img.close()
                w, h = w, h * min(frames, 50)
            pixels.append(w * h)
        return ValidationReport(metadata={"pixels": pixels})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        largest = max(metadata.get("pixels") or [0])
        return ResourceEstimate(
            ram_bytes=128 * MB + largest * 4 * 5,
            disk_bytes=sum(metadata.get("pixels") or [0]) * 4,
            cost_units=max(1, len(ctx.inputs)),
        )

    # ------------------------------------------------------------- execute

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = self.target(params)
        self.use_tiers(ctx, to)
        max_side = int(params.get("maxSide") or 0)
        outputs = []
        total = len(ctx.inputs)
        ctx.stage("convert", 1, 1)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            out = ctx.output_path(self.output_name(params, i, to))
            if to == "pdf":
                self._to_pdf(ctx, path, out, params, max_side)
            else:
                self._to_image(ctx, path, out, to, params, max_side)
            outputs.append(OutputFile(out, pages=1 if to == "pdf" else None))
            ctx.progress("convert", i + 1, total)
        return outputs

    def _to_image(
        self, ctx: JobContext, path: Path, out: Path, to: str, params: dict[str, Any], max_side: int
    ) -> None:
        src = source_format(path)
        keep_animation = to in ("gif", "webp", "avif")
        d = decode(ctx, path, max_side=max_side, first_only=not keep_animation)
        if d.animated and not keep_animation:
            ctx.warn("ANIMATION_FLATTENED")
        frames = [_fit(f, max_side) for f in d.frames]
        if max_side and frames[0].size != d.first.size and src != "svg":
            ctx.warn("IMAGE_DOWNSCALED", {"maxSide": max_side})
        quality = int(params.get("quality", 85))
        background = _hex(str(params.get("background", "#FFFFFF")))
        exif = _clean_exif(d.exif, bool(params.get("stripMetadata", False)))
        icc = d.icc
        save: dict[str, Any] = {}
        if to == "jpg":
            if any(_has_alpha(f) for f in frames[:1]):
                ctx.warn("TRANSPARENCY_FLATTENED")
            img = (
                _flatten(frames[0], background)
                if _has_alpha(frames[0])
                else frames[0].convert("RGB")
            )
            frames = [img]
            save = {"quality": quality, "optimize": True, "progressive": True}
        elif to == "bmp":
            if _has_alpha(frames[0]):
                ctx.warn("TRANSPARENCY_FLATTENED")
                frames = [_flatten(frames[0], background)]
            else:
                frames = [
                    frames[0].convert("RGB")
                    if frames[0].mode not in ("RGB", "L", "P", "1")
                    else frames[0]
                ]
            exif = None
        elif to == "png":
            frames = [_rgb_or_rgba(frames[0]) if frames[0].mode not in ("P", "1") else frames[0]]
            save = {"compress_level": 6}
        elif to == "tiff":
            frames = [
                frames[0]
                if frames[0].mode in ("RGB", "RGBA", "L", "LA", "1", "CMYK", "I;16")
                else _rgb_or_rgba(frames[0])
            ]
            save = {"compression": "tiff_lzw"}
        elif to == "webp":
            lossless = src not in LOSSY_CODED
            frames = [f.convert("RGBA" if _has_alpha(f) else "RGB") for f in frames]
            save = {"lossless": lossless, "quality": 100 if lossless else quality, "method": 4}
            if lossless:
                save["exact"] = True  # keep colour under transparent pixels
        elif to == "avif":
            frames = [f.convert("RGBA" if _has_alpha(f) else "RGB") for f in frames]
            # Speed 8 keeps one 50-megapixel encode well under the 2 s cancel budget.
            save = {"quality": quality, "speed": 8}
        elif to == "gif":
            if any(f.mode not in ("P", "L", "1") for f in frames):
                ctx.warn("COLORS_REDUCED")
            frames = [_to_palette(f) for f in frames]
            save = {"optimize": False}
            exif = icc = None
        elif to == "ico":
            img = _square(frames[0].convert("RGBA"))
            sizes = [s for s in ICO_SIZES if s <= max(img.size)] or [16]
            if max(frames[0].size) > 256:
                ctx.warn("IMAGE_DOWNSCALED", {"maxSide": 256})
            frames = [img]
            save = {"sizes": [(s, s) for s in sizes]}
            exif = icc = None
        if exif and to in ("jpg", "png", "webp", "tiff", "avif"):
            save["exif"] = exif
        if icc and to in ("jpg", "png", "webp", "tiff", "avif"):
            save["icc_profile"] = icc
        if len(frames) > 1:
            save.update(
                save_all=True,
                append_images=frames[1:],
                duration=d.durations[: len(frames)],
                loop=d.loop,
            )
        buf = io.BytesIO()
        try:
            frames[0].save(buf, PIL_FORMAT[to], **save)
        except (OSError, ValueError) as e:
            raise AdapterError(
                "E_INTERNAL", {"reason": "encode", "exception": type(e).__name__}
            ) from None
        ctx.check_cancelled()
        out.write_bytes(buf.getvalue())

    def _to_pdf(
        self, ctx: JobContext, path: Path, out: Path, params: dict[str, Any], max_side: int
    ) -> None:
        import pymupdf  # noqa: PLC0415

        src = source_format(path)
        if src == "svg":
            doc = _svg_document(path)
            try:
                data = doc.convert_to_pdf()
            finally:
                doc.close()
            pdf_bytes_to_output(ctx, data, out)
            return
        d = decode(ctx, path, max_side=max_side, first_only=True)
        if d.animated:
            ctx.warn("ANIMATION_FLATTENED")
        img = _fit(d.first, max_side)
        dpi = (d.dpi or (72, 72))[0] or 72
        width_pt, height_pt = img.width * 72 / dpi, img.height * 72 / dpi
        # JPEG goes in as-is (no re-encoding); everything else as lossless PNG.
        if (
            src == "jpg"
            and not max_side
            and img.size == d.first.size
            and d.first.mode in ("RGB", "L", "CMYK")
        ):
            stream = path.read_bytes()
        else:
            buf = io.BytesIO()
            _rgb_or_rgba(img).save(buf, "PNG")
            stream = buf.getvalue()
        doc = pymupdf.open()
        try:
            page = doc.new_page(width=width_pt, height=height_pt)
            page.insert_image(page.rect, stream=stream)
            data = doc.tobytes(garbage=3, deflate=True)
        finally:
            doc.close()
        pdf_bytes_to_output(ctx, data, out)


def _to_palette(img: Image.Image) -> Image.Image:
    if img.mode in ("P", "L", "1"):
        return img
    if _has_alpha(img):
        rgba = img.convert("RGBA")
        alpha = rgba.getchannel("A")
        p = rgba.convert("RGB").quantize(
            colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG
        )
        mask = alpha.point(lambda a: 255 if a < 128 else 0)
        p.paste(255, mask=mask)
        p.info["transparency"] = 255
        return p
    return img.convert("RGB").quantize(
        colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG
    )


def _square(img: Image.Image) -> Image.Image:
    side = max(img.size)
    if img.width == img.height:
        return img
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(img, ((side - img.width) // 2, (side - img.height) // 2))
    return canvas


if __name__ == "__main__":
    run_main(ImageConvertAdapter)
