"""pdf-to-image: every page (or chosen pages) of a PDF as images (ADR 0003)."""

from __future__ import annotations

import io
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import Tier, source_format
from converter_sdk.pdf import (
    OpenedInput,
    PdfAdapter,
    RangeError,
    pages_in,
    param_issue,
    parse_page_ranges,
)
from converter_sdk.protocol import ValidationIssue

MB = 1024 * 1024
MAX_PIXELS_PER_PAGE = 100_000_000
TARGETS: dict[str, Tier] = {
    "png": "H",
    "jpg": "L",
    "webp": "L",
    "tiff": "H",
    "gif": "L",
    "bmp": "H",
    "svg": "H",
}
EDGES: dict[tuple[str, str], Tier] = {("pdf", to): tier for to, tier in TARGETS.items()}
PIL_FORMAT = {
    "png": "PNG",
    "jpg": "JPEG",
    "webp": "WEBP",
    "tiff": "TIFF",
    "gif": "GIF",
    "bmp": "BMP",
}


class PdfToImageAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-to-image"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"
    EDGES = EDGES
    """For the manifest check: this is a conversion with these edges."""

    # PdfAdapter hooks -------------------------------------------------

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        to = str(params.get("to", ""))
        if to not in TARGETS:
            return [param_issue("to", "unsupportedConversion")]
        if any(source_format(o.path) != "pdf" for o in inputs):
            return [param_issue("to", "unsupportedConversion")]
        ranges = str(params.get("pages", "") or "")
        if ranges.strip():
            for o in inputs:
                try:
                    parse_page_ranges(ranges, o.pages)
                except RangeError as e:
                    return [param_issue("pages", e.reason)]
        return []

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        report = super().validate_input(ctx, params)
        # Reading pages changes nothing in the PDF: signature, form and
        # active-content warnings don't apply.
        report.warnings = []
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        dpi = int(params.get("dpi", 150))
        # A Letter page at this DPI, RGBA, a few copies at once.
        page_px = int(8.5 * dpi) * int(11 * dpi)
        pages = sum(int(p) for p in metadata.get("pages", []))
        return ResourceEstimate(
            ram_bytes=self.base_ram_bytes + 64 * MB + page_px * 4 * 4,
            disk_bytes=pages * page_px,
            cost_units=max(1, pages),
        )

    # execute ----------------------------------------------------------

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = str(params["to"])
        self.fidelity = TARGETS[to]  # type: ignore[misc]
        ranges = str(params.get("pages", "") or "")
        dpi = int(params.get("dpi", 150))
        outputs: list[OutputFile] = []
        ctx.stage("render", 1, 1)
        plans = []
        for path in ctx.inputs:
            count = self._page_count(ctx, path)
            pages = (
                list(pages_in(parse_page_ranges(ranges, count)))
                if ranges.strip()
                else list(range(1, count + 1))
            )
            plans.append((path, pages))
        total = sum(len(p) for _, p in plans)
        done = 0
        for index, (path, pages) in enumerate(plans):
            title = self._title(params, index)
            if to == "svg":
                outputs += self._svg(ctx, path, pages, title, index)
                done += len(pages)
                ctx.progress("render", done, total)
                continue
            images = []
            doc = self._open(ctx, path)
            for n in pages:
                ctx.check_cancelled()
                img = self._render(doc, n, dpi)
                if to == "tiff" and params.get("multipage", True) and len(pages) > 1:
                    images.append(img)
                else:
                    name = f"{title}.{to}" if len(pages) == 1 else f"{title}-page-{n}.{to}"
                    out = ctx.output_path(name)
                    self._save(img, out, to, params)
                    outputs.append(OutputFile(out, source=index))
                done += 1
                ctx.progress("render", done, total)
            doc.close()
            if images:
                out = ctx.output_path(f"{title}.tiff")
                buf = io.BytesIO()
                images[0].save(
                    buf, "TIFF", compression="tiff_lzw", save_all=True, append_images=images[1:]
                )
                out.write_bytes(buf.getvalue())
                outputs.append(OutputFile(out, source=index))
        return outputs

    @staticmethod
    def _title(params: dict[str, Any], index: int) -> str:
        titles = params.get("fileTitles") or []
        if index < len(titles) and isinstance(titles[index], str) and titles[index].strip():
            return titles[index].strip()
        return f"document-{index + 1}"

    def _password(self, ctx: JobContext) -> str | None:
        if ctx.secrets is not None and ctx.secrets.password is not None:
            return ctx.secrets.password.get_secret_value()
        return None

    def _page_count(self, ctx: JobContext, path: Path) -> int:
        doc = self._open(ctx, path)
        try:
            return len(doc)
        finally:
            doc.close()

    def _open(self, ctx: JobContext, path: Path):
        import pypdfium2 as pdfium  # noqa: PLC0415

        try:
            return pdfium.PdfDocument(path, password=self._password(ctx))
        except pdfium.PdfiumError:
            raise AdapterError("E_INPUT_CORRUPT") from None

    @staticmethod
    def _render(doc, page: int, dpi: int):
        p = doc[page - 1]
        w, h = p.get_size()
        scale = dpi / 72
        if w * h * scale * scale > MAX_PIXELS_PER_PAGE:
            scale = (MAX_PIXELS_PER_PAGE / (w * h)) ** 0.5
        return p.render(scale=scale, may_draw_forms=True).to_pil()

    def _save(self, img, out: Path, to: str, params: dict[str, Any]) -> None:
        quality = int(params.get("quality", 85))
        save: dict[str, Any] = {}
        if to in ("jpg", "bmp"):
            img = img.convert("RGB")
        if to == "jpg":
            save = {"quality": quality, "optimize": True, "progressive": True}
        elif to == "webp":
            save = {"quality": quality, "method": 4}
        elif to == "tiff":
            save = {"compression": "tiff_lzw"}
        elif to == "gif":
            img = img.convert("RGB").quantize(colors=256)
        buf = io.BytesIO()
        img.save(buf, PIL_FORMAT[to], **save)
        out.write_bytes(buf.getvalue())

    def _svg(
        self, ctx: JobContext, path: Path, pages: list[int], title: str, index: int
    ) -> list[OutputFile]:
        import pymupdf  # noqa: PLC0415

        doc = pymupdf.open(path)
        try:
            password = self._password(ctx)
            if doc.needs_pass and not (password and doc.authenticate(password)):
                raise AdapterError("E_PASSWORD_REQUIRED" if not password else "E_PASSWORD_WRONG")
            outputs = []
            for n in pages:
                ctx.check_cancelled()
                svg = doc[n - 1].get_svg_image(text_as_path=True)
                name = f"{title}.svg" if len(pages) == 1 else f"{title}-page-{n}.svg"
                out = ctx.output_path(name)
                out.write_text(svg, "utf-8", newline="\n")
                outputs.append(OutputFile(out, source=index))
            return outputs
        finally:
            doc.close()


if __name__ == "__main__":
    run_main(PdfToImageAdapter)
