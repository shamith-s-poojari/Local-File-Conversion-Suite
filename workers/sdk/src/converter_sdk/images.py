"""Shared Pillow helpers for the image tools (ADR 0004): opening with the
decompression-bomb guard, reading every frame, and writing an image back in
the format it came in.

Imported only by adapters that require Pillow; the SDK itself doesn't."""

from __future__ import annotations

import io
import warnings
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from PIL import Image, ImageOps, ImageSequence, UnidentifiedImageError

from .context import JobContext
from .convert import source_format
from .errors import AdapterError

MAX_PIXELS = 200_000_000
"""Larger images fail with E_LIMIT_EXCEEDED."""

PIL_FORMAT = {
    "png": "PNG", "jpg": "JPEG", "webp": "WEBP", "gif": "GIF", "bmp": "BMP",
    "tiff": "TIFF", "avif": "AVIF",
}  # fmt: skip
"""Formats the image tools can write back."""

WHITE = (255, 255, 255)


def open_image(path: Path) -> Image.Image:
    """Open a staged image, refusing decompression bombs."""
    if source_format(path) == "heic":
        try:
            import pillow_heif  # noqa: PLC0415 - optional engine

            pillow_heif.register_heif_opener()
        except ImportError:
            raise AdapterError("E_INSTALL_CORRUPT", {"missing": "pillow-heif"}) from None
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


def pixels(path: Path) -> int:
    """Width × height × frames (up to 50), for memory estimates."""
    img = open_image(path)
    try:
        return img.width * img.height * min(int(getattr(img, "n_frames", 1)), 50)
    finally:
        img.close()


@dataclass
class Frames:
    """An image as upright frames, with what's needed to write it back."""

    frames: list[Image.Image]
    durations: list[int] = field(default_factory=list)
    loop: int = 0
    exif: bytes | None = None
    icc: bytes | None = None

    @property
    def first(self) -> Image.Image:
        return self.frames[0]


def read_frames(ctx: JobContext, path: Path, *, first_only: bool = False) -> Frames:
    """Every frame (or the first), turned upright from the EXIF orientation."""
    img = open_image(path)
    try:
        info = dict(img.info)
        # A TIFF's "EXIF" is its own tag directory (sizes, strip offsets):
        # copying it into a new file would corrupt that file.
        upright_exif = None if img.format == "TIFF" else _without_orientation(img.getexif())
        total = int(getattr(img, "n_frames", 1))
        frames: list[Image.Image] = []
        durations: list[int] = []
        for i, frame in enumerate(ImageSequence.Iterator(img)):
            if first_only and i >= 1:
                break
            ctx.check_cancelled()
            frame.load()
            copy = frame.copy()
            if total == 1:
                copy = ImageOps.exif_transpose(copy)
            frames.append(copy)
            durations.append(int(frame.info.get("duration", info.get("duration", 100)) or 100))
        if not frames:
            raise AdapterError("E_INPUT_CORRUPT")
        return Frames(
            frames=frames,
            durations=durations,
            loop=int(info.get("loop", 0) or 0),
            exif=upright_exif,
            icc=info.get("icc_profile"),
        )
    except (OSError, SyntaxError, ValueError, EOFError):
        raise AdapterError("E_INPUT_CORRUPT") from None
    finally:
        img.close()


def _without_orientation(exif: Image.Exif) -> bytes | None:
    if not exif:
        return None
    try:
        exif.pop(0x0112, None)
        return exif.tobytes()
    except Exception:
        return None


def has_alpha(img: Image.Image) -> bool:
    if img.mode in ("RGBA", "LA", "PA"):
        return img.getextrema()[-1][0] < 255
    return img.mode == "P" and "transparency" in img.info


def flatten(img: Image.Image, background: tuple[int, int, int] = WHITE) -> Image.Image:
    """Transparent areas filled with ``background``."""
    rgba = img.convert("RGBA")
    base = Image.new("RGB", rgba.size, background)
    base.paste(rgba, mask=rgba.getchannel("A"))
    return base


def to_palette(img: Image.Image) -> Image.Image:
    """256 colours for GIF, keeping one transparent index if needed."""
    if img.mode in ("P", "L", "1"):
        return img
    if has_alpha(img):
        rgba = img.convert("RGBA")
        alpha = rgba.getchannel("A")
        p = rgba.convert("RGB").quantize(
            colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG
        )
        p.paste(255, mask=alpha.point(lambda a: 255 if a < 128 else 0))
        p.info["transparency"] = 255
        return p
    return img.convert("RGB").quantize(
        colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG
    )


def encode(
    image: Frames | Image.Image,
    fmt: str,
    *,
    quality: int = 90,
    keep_exif: bool = True,
    lossless_webp: bool = False,
) -> bytes:
    """``image`` written as ``fmt`` with sensible settings for each format.
    Animations stay animated in GIF and WebP."""
    f = image if isinstance(image, Frames) else Frames([image])
    frames = list(f.frames)
    save: dict[str, Any] = {}
    animated = len(frames) > 1 and fmt in ("gif", "webp")
    if not animated:
        frames = frames[:1]
    if fmt == "jpg":
        img = frames[0]
        frames = [flatten(img) if has_alpha(img) else img.convert("RGB")]
        save = {"quality": quality, "optimize": True, "progressive": True}
    elif fmt == "png":
        img = frames[0]
        if img.mode not in ("P", "1", "L", "LA", "RGB", "RGBA", "I;16"):
            img = img.convert("RGBA" if has_alpha(img) else "RGB")
        frames = [img]
        save = {"optimize": True}
    elif fmt == "bmp":
        img = frames[0]
        frames = [flatten(img) if has_alpha(img) else img.convert("RGB")]
    elif fmt == "tiff":
        img = frames[0]
        if img.mode not in ("RGB", "RGBA", "L", "LA", "1", "CMYK", "I;16"):
            img = img.convert("RGBA" if has_alpha(img) else "RGB")
        frames = [img]
        save = {"compression": "tiff_lzw"}
    elif fmt == "webp":
        frames = [x.convert("RGBA" if has_alpha(x) else "RGB") for x in frames]
        save = {
            "quality": 100 if lossless_webp else quality,
            "lossless": lossless_webp,
            "method": 4,
        }
    elif fmt == "avif":
        frames = [frames[0].convert("RGBA" if has_alpha(frames[0]) else "RGB")]
        save = {"quality": quality, "speed": 8}
    elif fmt == "gif":
        frames = [to_palette(x) for x in frames]
    else:
        raise AdapterError("E_INTERNAL", {"reason": "unsupportedFormat"}, retryable=False)
    if fmt in ("jpg", "gif"):
        # Pillow writes ``image.info["comment"]`` back out unless told not to,
        # which would carry a source file's comment into every output.
        save["comment"] = b""
    if keep_exif and f.exif and fmt in ("jpg", "png", "webp", "tiff", "avif"):
        save["exif"] = f.exif
    if f.icc and fmt in ("jpg", "png", "webp", "tiff", "avif"):
        save["icc_profile"] = f.icc
    if animated:
        save.update(
            save_all=True,
            append_images=frames[1:],
            duration=(f.durations or [100])[: len(frames)] or 100,
            loop=f.loop,
        )
    buf = io.BytesIO()
    try:
        frames[0].save(buf, PIL_FORMAT[fmt], **save)
    except (OSError, ValueError) as e:
        raise AdapterError(
            "E_INTERNAL", {"reason": "encode", "exception": type(e).__name__}
        ) from None
    return buf.getvalue()


__all__ = [
    "MAX_PIXELS",
    "PIL_FORMAT",
    "Frames",
    "encode",
    "flatten",
    "has_alpha",
    "open_image",
    "pixels",
    "read_frames",
    "to_palette",
]
