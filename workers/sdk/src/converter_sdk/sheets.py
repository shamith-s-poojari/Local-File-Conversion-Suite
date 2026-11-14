"""Pictures laid out on a grid: contact sheets of thumbnails with captions.

Shared by the contact-sheet and video-frames adapters. Imported only by
adapters that require Pillow; the SDK itself doesn't. Uses Pillow's built-in
font, so no font files ship."""

from __future__ import annotations

import math

from PIL import Image, ImageDraw, ImageFont

from .images import MAX_PIXELS, WHITE, flatten, has_alpha

Cell = tuple[Image.Image, "str | None"]
"""A thumbnail source and its caption (``None`` for no caption)."""

BORDER = (200, 200, 200)
TEXT = (40, 40, 40)
MIN_CELL_PX = 16


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


def font(size: int) -> ImageFont.ImageFont | ImageFont.FreeTypeFont:
    """Pillow's built-in font at ``size`` px (a fixed bitmap font if this
    Pillow has no FreeType)."""
    try:
        return ImageFont.load_default(size=size)
    except (TypeError, OSError):  # pragma: no cover - old Pillow or no FreeType
        return ImageFont.load_default()


def caption_size(cell_px: int) -> int:
    """Caption text height for cells ``cell_px`` wide."""
    return max(9, min(28, cell_px // 11))


def default_margin(cell_px: int) -> int:
    return max(6, cell_px // 14)


def cell_px_for_width(width: int, columns: int, margin: int | None = None) -> int:
    """Cell width that makes ``columns`` cells fill a sheet ``width`` px wide."""
    m = default_margin(width // max(1, columns)) if margin is None else margin
    gap = max(4, m // 2)
    return max(MIN_CELL_PX, (width - 2 * m - (columns - 1) * gap) // columns)


def _truncate(draw: ImageDraw.ImageDraw, text: str, fnt, max_w: int) -> str:
    if draw.textlength(text, font=fnt) <= max_w:
        return text
    while len(text) > 1 and draw.textlength(text + "...", font=fnt) > max_w:
        text = text[:-1]
    return text + "..."


class _Metrics:
    def __init__(
        self,
        n: int,
        columns: int,
        cell_px: int,
        aspect: float,
        captions: bool,
        title: bool,
        margin: int | None,
    ) -> None:
        self.cols = max(1, min(columns, max(1, n)))
        self.rows = max(1, math.ceil(n / self.cols))
        self.cell_w = max(MIN_CELL_PX, cell_px)
        self.cell_h = max(MIN_CELL_PX, round(self.cell_w / aspect))
        self.margin = default_margin(self.cell_w) if margin is None else margin
        self.gap = max(4, self.margin // 2)
        self.text = caption_size(self.cell_w)
        self.caption_h = round(self.text * 1.6) if captions else 0
        self.title_size = round(self.text * 1.5)
        self.title_h = round(self.title_size * 1.9) if title else 0
        self.step_y = self.cell_h + self.caption_h + self.gap
        self.width = 2 * self.margin + self.cols * self.cell_w + (self.cols - 1) * self.gap
        self.height = 2 * self.margin + self.title_h + self.rows * self.step_y - self.gap


def rows_per_page(
    height: int,
    cell_px: int,
    *,
    aspect: float = 1.0,
    captions: bool = True,
    title: bool = False,
    margin: int | None = None,
) -> int:
    """How many rows of cells fit on a sheet ``height`` px tall (at least 1)."""
    m = _Metrics(1, 1, cell_px, aspect, captions, title, margin)
    room = height - 2 * m.margin - m.title_h + m.gap
    return max(1, room // m.step_y)


def sheet_pages[T](cells: list[T], per_page: int) -> list[list[T]]:
    """``cells`` split into consecutive groups of at most ``per_page``; at
    least one (possibly empty) group, so there is always a sheet to make."""
    per_page = max(1, per_page)
    return [cells[i : i + per_page] for i in range(0, len(cells), per_page)] or [[]]


def contact_sheet(
    cells: list[Cell],
    *,
    columns: int,
    cell_px: int,
    title: str | None = None,
    margin: int | None = None,
    background: tuple[int, int, int] = WHITE,
    aspect: float = 1.0,
    min_size: tuple[int, int] | None = None,
) -> Image.Image:
    """Thumbnails in a grid, row by row, each with its caption underneath.

    ``cell_px`` is a cell's width and ``aspect`` its width over its height
    (1.0 for squares); a thumbnail keeps its proportions, fitted inside its
    cell and centred. ``min_size`` makes the sheet at least that big (for a
    fixed page). The sheet is kept under ``MAX_PIXELS`` by shrinking cells."""
    captions = any(c for _, c in cells)
    px = max(MIN_CELL_PX, cell_px)
    while True:
        m = _Metrics(len(cells), columns, px, aspect, captions, bool(title), margin)
        w = max(m.width, min_size[0] if min_size else 0)
        h = max(m.height, min_size[1] if min_size else 0)
        if w * h <= MAX_PIXELS or px <= MIN_CELL_PX:
            break
        px = max(MIN_CELL_PX, int(px * math.sqrt(MAX_PIXELS / (w * h)) * 0.98))
    sheet = Image.new("RGB", (w, h), background)
    draw = ImageDraw.Draw(sheet)
    small = font(m.text)
    x0 = (w - m.width) // 2 + m.margin
    y0 = m.margin
    if title:
        big = font(m.title_size)
        draw.text(
            (x0, y0 + m.title_h // 2),
            _truncate(draw, title, big, w - 2 * m.margin),
            fill=TEXT,
            font=big,
            anchor="lm",
        )
        y0 += m.title_h
    for i, (img, caption) in enumerate(cells):
        row, col = divmod(i, m.cols)
        x = x0 + col * (m.cell_w + m.gap)
        y = y0 + row * m.step_y
        thumb = _thumbnail(img, (m.cell_w, m.cell_h), background)
        tx, ty = x + (m.cell_w - thumb.width) // 2, y + (m.cell_h - thumb.height) // 2
        sheet.paste(thumb, (tx, ty))
        draw.rectangle([tx - 1, ty - 1, tx + thumb.width, ty + thumb.height], outline=BORDER)
        if caption and m.caption_h:
            text = _truncate(draw, caption, small, m.cell_w)
            draw.text(
                (x + m.cell_w // 2, y + m.cell_h + m.caption_h // 2 + 1),
                text,
                fill=TEXT,
                font=small,
                anchor="mm",
            )
    return sheet


def shrink(img: Image.Image, box: tuple[int, int], max_scale: float = 1.0) -> Image.Image:
    """``img`` scaled to fit inside ``box`` keeping its proportions, never
    enlarged by more than ``max_scale``. RGB, or RGBA when it has transparency.
    Shrink pictures right after reading them, so a sheet of many holds
    thumbnails and not full-size pictures."""
    w, h = box
    f = min(w / img.width, h / img.height, max_scale)
    if img.mode not in ("RGB", "RGBA", "L", "LA"):
        img = img.convert("RGBA" if has_alpha(img) or "A" in img.getbands() else "RGB")
    size = (max(1, round(img.width * f)), max(1, round(img.height * f)))
    if size == img.size:
        return img
    return img.resize(size, Image.Resampling.LANCZOS, reducing_gap=3.0)


def _thumbnail(
    img: Image.Image, size: tuple[int, int], background: tuple[int, int, int]
) -> Image.Image:
    """``img`` fitted inside ``size`` (small pictures are enlarged up to 4x),
    as RGB with any transparency filled with ``background``."""
    scaled = shrink(img, size, max_scale=4.0)
    return flatten(scaled, background) if has_alpha(scaled) else scaled.convert("RGB")
