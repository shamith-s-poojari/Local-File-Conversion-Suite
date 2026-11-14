"""pdf-page-numbers: page numbers, with optional text, on chosen pages
(ADR 0004). The text is drawn in Helvetica, which every PDF viewer has, so
nothing is embedded; it shows Western European characters only
(``GLYPH_UNSUPPORTED``, others appear as "?"). Numbers are placed in what
the viewer sees: the CropBox after /Rotate, as the watermark does."""

from __future__ import annotations

from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import JobContext, OutputFile, run_main
from converter_sdk.convert import ConversionAdapter
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
EDGE_Y = 28.0
"""Distance from the top or bottom edge to the text."""
EDGE_X = 36.0
STYLES = {
    "n": "{n}",
    "pageN": "Page {n}",
    "nOfTotal": "{n} of {total}",
    "pageNOfTotal": "Page {n} of {total}",
}
POSITIONS = ("bottomCenter", "bottomRight", "bottomLeft", "topCenter", "topRight", "topLeft")


def label(params: dict[str, Any], n: int, total: int) -> str:
    number = STYLES[params.get("style", "pageNOfTotal")].format(n=n, total=total)
    text = str(params.get("text") or "").strip()
    return f"{text}  ·  {number}" if text else number


def win_ansi(text: str) -> tuple[bytes, int]:
    """``text`` in WinAnsiEncoding, with "?" for characters it lacks; and how
    many distinct characters were replaced."""
    out = bytearray()
    missing: set[str] = set()
    for ch in text:
        try:
            out += ch.encode("cp1252")
        except UnicodeEncodeError:
            missing.add(ch)
            out += b"?"
    return bytes(out), len(missing)


def anchor(position: str, width: float, height: float, text_w: float) -> tuple[float, float]:
    """Where the text starts, in displayed coordinates."""
    y = EDGE_Y if position.startswith("bottom") else height - EDGE_Y
    if position.endswith("Left"):
        x = EDGE_X
    elif position.endswith("Right"):
        x = width - EDGE_X - text_w
    else:
        x = (width - text_w) / 2
    return x, y


def _free_name(page: pikepdf.Page) -> pikepdf.Name:
    """A font resource name the page doesn't use (deterministic)."""
    resources = page.obj.get(Name.Resources)
    fonts = resources.get(Name.Font) if isinstance(resources, pikepdf.Dictionary) else None
    used = set(fonts.keys()) if isinstance(fonts, pikepdf.Dictionary) else set()
    n = 0
    while f"/LfcsPn{n}" in used:
        n += 1
    return pikepdf.Name(f"/LfcsPn{n}")


class PdfPageNumbersAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-page-numbers"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"
    base_ram_bytes: ClassVar[int] = 128 * MB  # pikepdf plus ReportLab's metrics

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        if len(inputs) != 1:
            return [param_issue("inputs", "exactlyOne")]
        if params.get("style", "pageNOfTotal") not in STYLES:
            return [param_issue("style", "invalid")]
        if params.get("position", "bottomCenter") not in POSITIONS:
            return [param_issue("position", "invalid")]
        if params.get("pages"):
            try:
                parse_page_ranges(params["pages"], inputs[0].pages)
            except RangeError as e:
                return [param_issue("pages", e.reason, pages=inputs[0].pages)]
        return []

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        from reportlab.pdfbase.pdfmetrics import stringWidth  # noqa: PLC0415 - heavy import

        size = float(params.get("fontSize", 10))
        start = int(params.get("startAt", 1))
        position = params.get("position", "bottomCenter")
        replaced = 0
        with ctx.open_pdf(ctx.inputs[0]) as pdf:
            self.warn_shared(ctx, [pdf])
            selected = (
                pages_in(parse_page_ranges(params["pages"], len(pdf.pages)))
                if params.get("pages")
                else list(range(1, len(pdf.pages) + 1))
            )
            total = start + len(selected) - 1
            font = pdf.make_indirect(
                pikepdf.Dictionary(
                    Type=Name.Font,
                    Subtype=Name.Type1,
                    BaseFont=Name.Helvetica,
                    Encoding=Name.WinAnsiEncoding,
                )
            )
            ctx.stage("number", 1, 2)
            for i, number in enumerate(selected):
                ctx.check_cancelled()
                page = pdf.pages[number - 1]
                encoded, missing = win_ansi(label(params, start + i, total))
                replaced = max(replaced, missing)
                x0, y0, w, h, rotate = displayed_box(page)
                shown_w, shown_h = (h, w) if rotate in (90, 270) else (w, h)
                text_w = stringWidth(encoded.decode("cp1252"), "Helvetica", size)
                x, y = anchor(position, shown_w, shown_h, text_w)
                name = _free_name(page)
                page.add_resource(font, Name.Font, name)
                draw = pikepdf.unparse_content_stream(
                    [
                        ([], pikepdf.Operator("q")),
                        (placement(x0, y0, w, h, rotate), pikepdf.Operator("cm")),
                        ([0], pikepdf.Operator("g")),
                        ([], pikepdf.Operator("BT")),
                        ([name, size], pikepdf.Operator("Tf")),
                        ([x, y - size / 3], pikepdf.Operator("Td")),
                        ([pikepdf.String(encoded)], pikepdf.Operator("Tj")),
                        ([], pikepdf.Operator("ET")),
                        ([], pikepdf.Operator("Q")),
                    ]
                )
                # Wrapped so the page's own drawing state can't leak into ours.
                page.contents_add(pdf.make_stream(b"q\n"), prepend=True)
                page.contents_add(pdf.make_stream(b"Q\n" + draw + b"\n"))
                ctx.progress("number", i + 1, len(selected))
            if replaced:
                ctx.warn("GLYPH_UNSUPPORTED", {"characters": replaced})
            ctx.stage("write", 2, 2)
            title = ConversionAdapter.title(params, 0, "document")
            out = ctx.output_path(f"{title}-numbered.pdf")
            pages = save_pdf(ctx, pdf, out)
            return [OutputFile(out, pages=pages)]


if __name__ == "__main__":
    run_main(PdfPageNumbersAdapter)
