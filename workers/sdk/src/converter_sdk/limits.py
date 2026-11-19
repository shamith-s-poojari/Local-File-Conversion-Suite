"""Size limits shared by every adapter, with no engine imported.

One place decides how large a raster may get, so a preview, a conversion and
a redaction fallback can't each pick their own number.
"""

from __future__ import annotations

import math

from .errors import AdapterError

MB = 1024 * 1024

MAX_PIXELS = 200_000_000
"""Largest single raster, input or output. Pillow's own limit is set to match."""

MAX_FRAMES = 500
"""Most frames an animated image may have."""

MAX_ANIMATION_PIXELS = 100_000_000
"""Frames × width × height for an animation. Frames are all held in memory
at once, so this is stricter than ``MAX_PIXELS``."""


def check_pixels(width: int, height: int, frames: int = 1) -> None:
    """Refuse a raster (or animation) over the limits with ``E_LIMIT_EXCEEDED``.

    Call it with the *declared* size before decoding or rendering anything:
    the size comes from a header, and the decoder allocates from it."""
    if width < 0 or height < 0 or frames < 0:
        raise AdapterError("E_INPUT_CORRUPT")
    if width * height > MAX_PIXELS:
        raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "pixels"})
    if frames > MAX_FRAMES:
        raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "frames", "max": MAX_FRAMES})
    if frames > 1 and frames * width * height > MAX_ANIMATION_PIXELS:
        raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "framePixels"})


def fit_scale(width: float, height: float, scale: float, limit: int | None = None) -> float:
    """``scale`` lowered, if needed, so a ``width`` × ``height`` page rendered
    at it stays within ``limit`` pixels. Sizes are in the page's own units."""
    limit = MAX_PIXELS if limit is None else limit
    area = max(width, 0.0) * max(height, 0.0)
    if area <= 0 or area * scale * scale <= limit:
        return scale
    fitted = math.sqrt(limit / area) * 0.999
    # Each side is rounded up when rendered, which can push a small limit over.
    for _ in range(200):
        w, h = rendered_size(width, height, fitted)
        if w * h <= limit:
            break
        fitted *= 0.99
    return min(scale, fitted)


def rendered_size(width: float, height: float, scale: float) -> tuple[int, int]:
    """The pixel size a renderer gives a page of ``width`` × ``height`` at ``scale``."""
    return max(1, math.ceil(width * scale)), max(1, math.ceil(height * scale))


__all__ = [
    "MAX_ANIMATION_PIXELS",
    "MAX_FRAMES",
    "MAX_PIXELS",
    "MB",
    "check_pixels",
    "fit_scale",
    "rendered_size",
]
