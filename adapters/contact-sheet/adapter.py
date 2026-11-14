"""contact-sheet: thumbnails of many pictures and PDF pages on sheets, with
names under them, as pictures or one PDF (ADR 0004). A picture gives one
thumbnail (its first frame, upright); a PDF gives one per page. Sheets are
built one at a time so memory doesn't grow with the number of files."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
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
from converter_sdk.convert import ConversionAdapter, issue, source_format
from converter_sdk.images import pixels, read_frames
from converter_sdk.protocol import ValidationIssue
from converter_sdk.sheets import (
    cell_px_for_width,
    contact_sheet,
    default_margin,
    rows_per_page,
    sheet_pages,
    shrink,
)
from PIL import Image

MB = 1024 * 1024
MAX_PDF_PAGES = 500
"""Per PDF."""
MAX_CELLS = 2000
"""Thumbnails in all."""
SHEET_PX: dict[str, tuple[int, int | None]] = {
    "a4": (1240, 1754),  # 150 dpi
    "letter": (1275, 1650),
    "screen": (1920, None),  # as tall as its rows need
}
SCREEN_ROWS = 10
PAGE_PT = {"a4": (595.28, 841.89), "letter": (612.0, 792.0)}
PX_PT = 0.75
"""Points per pixel for a screen sheet saved as a PDF (96 dpi)."""
TARGETS = ("png", "jpg", "pdf")


@dataclass
class Cell:
    path: Path
    page: int | None
    caption: str


@dataclass
class Geometry:
    width: int
    height: int | None
    columns: int
    cell_px: int
    margin: int
    per_page: int
    captions: bool


def geometry(params: dict[str, Any]) -> Geometry:
    width, height = SHEET_PX[str(params.get("sheet", "a4"))]
    columns = int(params.get("columns", 4))
    captions = bool(params.get("captions", True))
    margin = default_margin(width // columns)
    cell_px = cell_px_for_width(width, columns, margin)
    rows = (
        SCREEN_ROWS
        if height is None
        else rows_per_page(height, cell_px, captions=captions, margin=margin)
    )
    return Geometry(width, height, columns, cell_px, margin, columns * rows, captions)


class ContactSheetAdapter(Adapter):
    adapter_id: ClassVar[str] = "contact-sheet"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    # ------------------------------------------------------------ validate

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        if params.get("to") not in TARGETS:
            return ValidationReport(errors=[issue("to", "unsupported")])
        if params.get("sheet", "a4") not in SHEET_PX:
            return ValidationReport(errors=[issue("sheet", "invalid")])
        if not 3 <= int(params.get("columns", 4)) <= 8:
            return ValidationReport(errors=[issue("columns", "invalid")])
        sizes: list[int] = []
        cells = 0
        for path in ctx.inputs:
            ctx.check_cancelled()
            if source_format(path) != "pdf":
                sizes.append(pixels(path))
                cells += 1
                continue
            relative = path.relative_to(ctx.job_dir.resolve()).as_posix()
            try:
                with ctx.open_pdf(path) as pdf:
                    pages = len(pdf.pages)
            except AdapterError as e:
                return ValidationReport(errors=[ValidationIssue(code=e.code, input=relative)])
            if pages == 0:
                return ValidationReport(
                    errors=[ValidationIssue(code="E_INPUT_CORRUPT", input=relative)]
                )
            if pages > MAX_PDF_PAGES:
                return ValidationReport(
                    errors=[
                        ValidationIssue(
                            code="E_LIMIT_EXCEEDED",
                            input=relative,
                            detail={"limit": "pages", "max": MAX_PDF_PAGES},
                        )
                    ]
                )
            cells += pages
        if cells > MAX_CELLS:
            return ValidationReport(
                errors=[
                    ValidationIssue(
                        code="E_LIMIT_EXCEEDED", detail={"limit": "thumbnails", "max": MAX_CELLS}
                    )
                ]
            )
        return ValidationReport(metadata={"pixels": sizes, "cells": cells})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        g = geometry(params)
        cells = int(metadata.get("cells") or 1)
        largest = max(metadata.get("pixels") or [0])
        sheet_h = g.height or g.width * 4  # a screen sheet is at most ten rows
        sheet_px = g.width * sheet_h
        sheets = -(-cells // g.per_page)
        return ResourceEstimate(
            ram_bytes=160 * MB
            + largest * 4 * 3
            + sheet_px * 3 * 3
            + min(cells, g.per_page) * g.cell_px * g.cell_px * 4
            + (g.cell_px * 2) ** 2 * 4,
            disk_bytes=sheets * sheet_px * 3 + MB,
            cost_units=max(1, cells),
        )

    # ------------------------------------------------------------- execute

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = str(params["to"])
        g = geometry(params)
        ctx.stage("sheets", 1, 1)
        cells = self.plan(ctx, params)
        groups = sheet_pages(cells, g.per_page)
        total = len(cells)
        done = 0
        files: list[Path] = []
        with _Pages(ctx) as pages:
            for k, group in enumerate(groups):
                thumbs = []
                for cell in group:
                    ctx.check_cancelled()
                    thumbs.append(
                        (self.thumbnail(pages, cell, g), cell.caption if g.captions else None)
                    )
                    done += 1
                    ctx.progress("sheets", done, total)
                sheet = contact_sheet(
                    thumbs,
                    columns=g.columns,
                    cell_px=g.cell_px,
                    margin=g.margin,
                    min_size=(g.width, g.height or 0),
                )
                del thumbs
                if to == "pdf":
                    tmp = ctx.temp_path(".jpg")
                    sheet.save(tmp, "JPEG", quality=90)
                    files.append(tmp)
                else:
                    name = "contact-sheet" if len(groups) == 1 else f"contact-sheet-{k + 1}"
                    out = ctx.output_path(f"{name}.{to}")
                    if to == "jpg":
                        sheet.save(out, "JPEG", quality=90, optimize=True)
                    else:
                        sheet.save(out, "PNG")
                    files.append(out)
        source = 0 if len(ctx.inputs) == 1 else None
        if to == "pdf":
            return [self.assemble(ctx, files, params, source)]
        return [OutputFile(f, source=source) for f in files]

    def plan(self, ctx: JobContext, params: dict[str, Any]) -> list[Cell]:
        cells: list[Cell] = []
        for i, path in enumerate(ctx.inputs):
            fmt = source_format(path)
            title = ConversionAdapter.title(params, i)
            if fmt != "pdf":
                cells.append(Cell(path, None, f"{title}.{fmt}"))
                continue
            count = _Pages.count(ctx, path)
            cells += [Cell(path, n, f"{title} p. {n}") for n in range(1, count + 1)]
        return cells

    @staticmethod
    def thumbnail(pages: _Pages, cell: Cell, g: Geometry) -> Image.Image:
        if cell.page is None:
            return shrink(
                read_frames(pages.ctx, cell.path, first_only=True).first, (g.cell_px,) * 2
            )
        return pages.render(cell.path, cell.page, g.cell_px)

    @staticmethod
    def assemble(
        ctx: JobContext, images: list[Path], params: dict[str, Any], source: int | None
    ) -> OutputFile:
        """A PDF with each sheet picture filling a page."""
        from reportlab.lib.utils import ImageReader  # noqa: PLC0415
        from reportlab.pdfgen.canvas import Canvas  # noqa: PLC0415

        out = ctx.output_path("contact-sheet.pdf")
        sheet = str(params.get("sheet", "a4"))
        canvas = Canvas(str(out), pageCompression=1, invariant=1)
        canvas.setTitle("Contact sheet")
        for path in images:
            ctx.check_cancelled()
            with Image.open(path) as probe:
                px = probe.size
            w, h = PAGE_PT.get(sheet) or (px[0] * PX_PT, px[1] * PX_PT)
            canvas.setPageSize((w, h))
            canvas.drawImage(ImageReader(str(path)), 0, 0, w, h)
            canvas.showPage()
        canvas.save()
        return OutputFile(out, pages=len(images), source=source)


class _Pages:
    """The PDF being read, kept open while its pages are taken in turn."""

    def __init__(self, ctx: JobContext) -> None:
        self.ctx = ctx
        self.path: Path | None = None
        self.doc: Any = None

    def __enter__(self) -> _Pages:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def close(self) -> None:
        if self.doc is not None:
            self.doc.close()
        self.doc, self.path = None, None

    @staticmethod
    def open(ctx: JobContext, path: Path) -> Any:
        import pypdfium2 as pdfium  # noqa: PLC0415

        password = None
        if ctx.secrets is not None and ctx.secrets.password is not None:
            password = ctx.secrets.password.get_secret_value()
        try:
            return pdfium.PdfDocument(path, password=password)
        except pdfium.PdfiumError:
            ctx.open_pdf(path).close()  # raises the password errors
            raise AdapterError("E_INPUT_CORRUPT") from None
        finally:
            del password

    @staticmethod
    def count(ctx: JobContext, path: Path) -> int:
        doc = _Pages.open(ctx, path)
        try:
            return len(doc)
        finally:
            doc.close()

    def render(self, path: Path, page: int, cell_px: int) -> Image.Image:
        """Page ``page`` (1-based) small enough to fit a cell."""
        if self.path != path:
            self.close()
            self.doc, self.path = self.open(self.ctx, path), path
        p = self.doc[page - 1]
        try:
            w, h = p.get_size()
            fit = min(cell_px / w, cell_px / h)
            scale = max(0.05, min(1.0, fit * 1.5))  # up to 72 dpi
            return shrink(p.render(scale=scale).to_pil(), (cell_px, cell_px))
        finally:
            p.close()


if __name__ == "__main__":
    run_main(ContactSheetAdapter)
