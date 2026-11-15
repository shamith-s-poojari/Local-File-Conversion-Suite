"""pdf-watermark: stamp text on pages (spec section 13.7).

One stamp PDF is built per distinct displayed page size and shared by every
page of that size, so a 5,000-page job grows the file only slightly. The
stamp is placed in what the viewer sees: the CropBox after /Rotate.
"""

from __future__ import annotations

import io
import math
from pathlib import Path
from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import JobContext, OutputFile, run_main
from converter_sdk.pdf import (
    OpenedInput,
    PdfAdapter,
    RangeError,
    displayed_box,
    pages_in,
    param_issue,
    parse_page_ranges,
    placement,
    save_pdf,
)
from converter_sdk.protocol import ValidationIssue
from pikepdf import Name

MB = 1024 * 1024
MARGIN = 36.0
POSITIONS = ("center", "topLeft", "topRight", "bottomLeft", "bottomRight", "tile")
FONT_FILE = Path(__file__).resolve().parents[2] / "engines" / "fonts" / "NotoSans-Regular.ttf"
"""Bundled by packaging (spec 13.7). Without it, Helvetica is used and only
Windows-1252 characters can be shown."""


def _font() -> tuple[str, Any]:
    """Register the watermark font; returns (name, glyph test)."""
    from reportlab.pdfbase import pdfmetrics  # noqa: PLC0415 - heavy import, execute only
    from reportlab.pdfbase.ttfonts import TTFont  # noqa: PLC0415

    if FONT_FILE.is_file():
        font = TTFont("WatermarkSans", str(FONT_FILE))
        pdfmetrics.registerFont(font)
        cmap = font.face.charToGlyph
        return "WatermarkSans", lambda ch: ord(ch) in cmap

    def in_cp1252(ch: str) -> bool:
        try:
            ch.encode("cp1252")
        except UnicodeEncodeError:
            return False
        return True

    return "Helvetica", in_cp1252


def _rgb(color: str) -> tuple[float, float, float]:
    return tuple(int(color[i : i + 2], 16) / 255 for i in (1, 3, 5))  # type: ignore[return-value]


def make_stamp(width: float, height: float, params: dict[str, Any], font: str, text: str) -> bytes:
    from reportlab.pdfbase.pdfmetrics import stringWidth  # noqa: PLC0415
    from reportlab.pdfgen import canvas  # noqa: PLC0415

    size = float(params.get("fontSize", 48))
    angle = float(params.get("rotation", 45))
    position = params.get("position", "center")
    buf = io.BytesIO()
    c = canvas.Canvas(buf, pagesize=(width, height), invariant=1, pageCompression=1)
    c.setFillColorRGB(*_rgb(params.get("color", "#808080")))
    c.setFillAlpha(float(params.get("opacity", 0.3)))
    c.setFont(font, size)

    text_w = stringWidth(text, font, size)
    rad = math.radians(angle)
    hx = (abs(text_w * math.cos(rad)) + abs(size * math.sin(rad))) / 2
    hy = (abs(text_w * math.sin(rad)) + abs(size * math.cos(rad))) / 2

    def draw(cx: float, cy: float) -> None:
        c.saveState()
        c.translate(cx, cy)
        c.rotate(angle)
        c.drawCentredString(0, -size / 3, text)
        c.restoreState()

    anchors = {
        "center": (width / 2, height / 2),
        "topLeft": (MARGIN + hx, height - MARGIN - hy),
        "topRight": (width - MARGIN - hx, height - MARGIN - hy),
        "bottomLeft": (MARGIN + hx, MARGIN + hy),
        "bottomRight": (width - MARGIN - hx, MARGIN + hy),
    }
    if position == "tile":
        step_x = max(2 * hx + 72, 144.0)
        step_y = max(2 * hy + 72, 144.0)
        y = step_y / 2
        while y < height + step_y:
            x = step_x / 2
            while x < width + step_x:
                draw(x, y)
                x += step_x
            y += step_y
    else:
        draw(*anchors[position])
    c.showPage()
    c.save()
    return buf.getvalue()


def _free_name(page: pikepdf.Page) -> pikepdf.Name:
    """A resource name not used on this page. Deterministic, so repeat runs
    give identical output (pikepdf's ``prefix`` option adds a random suffix)."""
    resources = page.obj.get(Name.Resources)
    xobjects = resources.get(Name.XObject) if isinstance(resources, pikepdf.Dictionary) else None
    used = set(xobjects.keys()) if isinstance(xobjects, pikepdf.Dictionary) else set()
    n = 0
    while f"/LfcsWm{n}" in used:
        n += 1
    return pikepdf.Name(f"/LfcsWm{n}")


class PdfWatermarkAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-watermark"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"
    base_ram_bytes: ClassVar[int] = 128 * MB  # pikepdf plus ReportLab

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        if len(inputs) != 1:
            return [param_issue("inputs", "exactlyOne")]
        if params.get("type", "text") != "text":
            return [param_issue("type", "notImplemented")]
        text = params.get("text")
        if not isinstance(text, str) or not text.strip() or len(text) > 500:
            return [param_issue("text", "required")]
        if params.get("position", "center") not in POSITIONS:
            return [param_issue("position", "invalid")]
        if params.get("layer", "over") not in ("over", "under"):
            return [param_issue("layer", "invalid")]
        if not 0.05 <= float(params.get("opacity", 0.3)) <= 1.0:
            return [param_issue("opacity", "outOfBounds")]
        if not 4 <= float(params.get("fontSize", 48)) <= 400:
            return [param_issue("fontSize", "outOfBounds")]
        color = params.get("color", "#808080")
        if not (isinstance(color, str) and len(color) == 7 and color[0] == "#"):
            return [param_issue("color", "invalid")]
        try:
            int(color[1:], 16)
        except ValueError:
            return [param_issue("color", "invalid")]
        if params.get("pages"):
            try:
                parse_page_ranges(params["pages"], inputs[0].pages)
            except RangeError as e:
                return [param_issue("pages", e.reason, pages=inputs[0].pages)]
        return []

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        font, has_glyph = _font()
        text = params["text"]
        missing = sum(1 for ch in set(text) if not ch.isspace() and not has_glyph(ch))
        if missing:
            ctx.warn("GLYPH_UNSUPPORTED", {"characters": missing})
            text = "".join(ch if ch.isspace() or has_glyph(ch) else "?" for ch in text)
        under = params.get("layer", "over") == "under"

        stamps: list[pikepdf.Pdf] = []
        try:
            with ctx.open_pdf(ctx.inputs[0]) as pdf:
                self.warn_shared(ctx, [pdf])
                total = len(pdf.pages)
                selected = (
                    pages_in(parse_page_ranges(params["pages"], total))
                    if params.get("pages")
                    else list(range(1, total + 1))
                )
                cache: dict[tuple[float, float], pikepdf.Object] = {}
                ctx.stage("stamp", 1, 2)
                for done, number in enumerate(selected, start=1):
                    ctx.check_cancelled()
                    page = pdf.pages[number - 1]
                    x0, y0, w, h, rotate = displayed_box(page)
                    shown = (h, w) if rotate in (90, 270) else (w, h)
                    key = (round(shown[0], 2), round(shown[1], 2))
                    if key not in cache:
                        stamp = pikepdf.open(io.BytesIO(make_stamp(*shown, params, font, text)))
                        stamps.append(stamp)
                        xobject = stamp.pages[0].as_form_xobject()
                        cache[key] = pdf.copy_foreign(xobject)
                    name = _free_name(page)
                    page.add_resource(cache[key], Name.XObject, name)
                    cm = " ".join(f"{v:g}" for v in placement(x0, y0, w, h, rotate))
                    draw = f"q {cm} cm {name} Do Q\n".encode("ascii")
                    if under:
                        page.contents_add(pdf.make_stream(draw), prepend=True)
                    else:
                        page.contents_add(pdf.make_stream(b"q\n"), prepend=True)
                        page.contents_add(pdf.make_stream(b"Q\n" + draw))
                    ctx.progress("stamp", done, len(selected))

                ctx.stage("write", 2, 2)
                out = ctx.output_path("watermarked.pdf")
                pages = save_pdf(ctx, pdf, out)
                return [OutputFile(out, pages=pages)]
        finally:
            for stamp in stamps:
                stamp.close()


if __name__ == "__main__":
    run_main(PdfWatermarkAdapter)
