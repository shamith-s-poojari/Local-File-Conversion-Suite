"""office-convert: Word, PowerPoint, Excel, OpenDocument, RTF, CSV, plain
text, HTML and Markdown through LibreOffice (ADR 0003).

Every conversion is one job here; the ones that need a step in between
(a document to PNG goes through PDF, Markdown to Word through HTML) do it
internally, in ``work/``.
"""

from __future__ import annotations

import io
import re
import shutil
from pathlib import Path
from typing import Any, ClassVar

from converter_sdk import (
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import ConversionAdapter, Tier, pdf_bytes_to_output, source_format
from converter_sdk.markup import html_to_markdown, markdown_to_html, read_text
from converter_sdk.office import IMPORT_FILTERS, export_filter, soffice_convert
from converter_sdk.protocol import ValidationIssue

MB = 1024 * 1024
WRITER = ["docx", "doc", "odt", "rtf"]
IMPRESS = ["pptx", "ppt", "odp"]
CALC = ["xlsx", "xls", "ods"]

EDGES: dict[tuple[str, str], Tier] = {}
for _s in WRITER:
    for _t, _tier in [("pdf", "H"), ("docx", "H"), ("doc", "H"), ("odt", "H"), ("rtf", "H"),
                      ("txt", "L"), ("html", "R"), ("md", "R"), ("epub", "R"), ("png", "H"),
                      ("jpg", "L"), ("webp", "L")]:  # fmt: skip
        if _s != _t:
            EDGES[(_s, _t)] = _tier
for _s in IMPRESS:
    for _t, _tier in [("pdf", "H"), ("pptx", "H"), ("ppt", "H"), ("odp", "H"),
                      ("txt", "L"), ("png", "H"), ("jpg", "L"), ("webp", "L")]:  # fmt: skip
        if _s != _t:
            EDGES[(_s, _t)] = _tier
for _s in CALC:
    for _t, _tier in [("pdf", "H"), ("xlsx", "H"), ("xls", "H"), ("ods", "H"), ("csv", "L"),
                      ("html", "R"), ("txt", "L"), ("png", "H"), ("jpg", "L"),
                      ("webp", "L")]:  # fmt: skip
        if _s != _t:
            EDGES[(_s, _t)] = _tier
EDGES.update({("csv", t): "H" for t in ["xlsx", "xls", "ods", "pdf"]})
# Text, HTML and Markdown to each other (and to PDF) are markup-convert's,
# which needs no LibreOffice; these are the ones that need a word processor.
EDGES.update({("txt", t): "H" for t in ["pdf", "docx", "odt", "rtf", "html"]})
EDGES.update(
    {("html", "pdf"): "H", ("html", "docx"): "R", ("html", "odt"): "R", ("html", "txt"): "L"}
)
EDGES.update({("md", t): "H" for t in ["docx", "odt", "rtf"]})
EDGES[("md", "epub")] = "R"
# PDF pages become slides of editable text and shapes (Impress's PDF import).
EDGES.update({("pdf", "pptx"): "R", ("pdf", "odp"): "R"})

TEXT_SHEETS = "txt:Text - txt - csv (StarCalc):9,34,76,1,,0,false,true,true,false,false,-1"
"""Tab-separated, UTF-8, values as shown, every sheet."""

IMAGE_TARGETS = {"png", "jpg", "webp"}
PIL_FORMAT = {"png": "PNG", "jpg": "JPEG", "webp": "WEBP"}


class OfficeConvertAdapter(ConversionAdapter):
    adapter_id: ClassVar[str] = "office-convert"
    EDGES = EDGES

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        sizes = []
        report = ValidationReport()
        for p in ctx.inputs:
            size = p.stat().st_size
            if size == 0:
                raise AdapterError("E_INPUT_CORRUPT")
            sizes.append(size)
            if source_format(p) == "pdf":
                try:
                    self._plain_pdf(ctx, p, check_only=True)
                except AdapterError as e:
                    rel = p.relative_to(ctx.job_dir.resolve()).as_posix()
                    report.errors.append(ValidationIssue(code=e.code, input=rel))
        report.metadata = {"bytes": sizes}
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        largest = max(metadata.get("bytes") or [0])
        # Headless LibreOffice peaks near 800 MB of physical footprint on its
        # first start with a fresh profile (measured on macOS 26), plus Python.
        return ResourceEstimate(
            ram_bytes=900 * MB + largest * 20, disk_bytes=largest * 10 + 80 * MB, cost_units=10
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = self.target(params)
        self.use_tiers(ctx, to)
        outputs: list[OutputFile] = []
        total = len(ctx.inputs)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("convert", i + 1, total)
            ctx.progress("convert", i, total)
            outputs += self._one(ctx, path, source_format(path), to, params, i)
        ctx.progress("convert", total, total)
        return outputs

    # ---------------------------------------------------------------- steps

    def _one(
        self, ctx: JobContext, path: Path, src: str, to: str, params: dict[str, Any], i: int
    ) -> list[OutputFile]:
        title = self.title(params, i)
        source, import_filter, writer_src = path, None, src
        if src == "md":
            # Markdown → HTML → LibreOffice Writer.
            source = ctx.temp_path(".html")
            source.write_text(markdown_to_html(read_text(path), title), "utf-8", newline="\n")
            import_filter, writer_src = IMPORT_FILTERS["html"], "html"
        elif src == "pdf":
            source = self._plain_pdf(ctx, path)
            import_filter, writer_src = "impress_pdf_import", "pptx"
            ctx.warn("LAYOUT_APPROXIMATED")
        elif src in IMPORT_FILTERS:
            import_filter = IMPORT_FILTERS[src]

        if to == "txt" and src in CALC:
            produced = soffice_convert(ctx, source, TEXT_SHEETS, import_filter=import_filter)
            ctx.warn("FORMULAS_DROPPED")
            parts = []
            for p in produced:
                sheet = _sheet_name(p, path.stem)
                body = read_text(p).rstrip("\n")
                parts.append(f"{sheet}\n\n{body}" if len(produced) > 1 and sheet else body)
            return [self._write_text(ctx, f"{title}.txt", "\n\n\n".join(parts) + "\n")]

        if to in IMAGE_TARGETS or (to == "txt" and src in IMPRESS):
            pdf = soffice_convert(
                ctx, source, export_filter(writer_src, "pdf"), import_filter=import_filter
            )[0]
            return self._from_pdf(ctx, pdf, to, title, params)
        if to == "md":
            html = soffice_convert(
                ctx, source, export_filter(writer_src, "html"), import_filter=import_filter
            )[0]
            md, images = html_to_markdown(read_text(html))
            if images:
                ctx.warn("IMAGES_DROPPED")
            ctx.warn("LAYOUT_APPROXIMATED")
            return [self._write_text(ctx, f"{title}.md", md)]

        produced = soffice_convert(
            ctx, source, export_filter(writer_src, to), import_filter=import_filter
        )
        if to == "pdf":
            out = ctx.output_path(f"{title}.pdf")
            pages = pdf_bytes_to_output(ctx, produced[0].read_bytes(), out)
            return [OutputFile(out, pages=pages)]
        if to == "csv":
            ctx.warn("FORMULAS_DROPPED")
            if len(produced) > 1:
                ctx.warn("SHEETS_SPLIT")
            outs = []
            for p in produced:
                sheet = _sheet_name(p, path.stem)
                name = f"{title}-{sheet}.csv" if len(produced) > 1 and sheet else f"{title}.csv"
                out = ctx.output_path(name)
                shutil.copyfile(p, out)
                outs.append(OutputFile(out))
            return outs
        if to in ("html", "epub", "txt"):
            if to != "txt":
                ctx.warn("LAYOUT_APPROXIMATED")
        out = ctx.output_path(f"{title}.{to}")
        shutil.copyfile(produced[0], out)
        return [OutputFile(out)]

    def _from_pdf(
        self, ctx: JobContext, pdf: Path, to: str, title: str, params: dict[str, Any]
    ) -> list[OutputFile]:
        import pypdfium2 as pdfium  # noqa: PLC0415

        doc = pdfium.PdfDocument(pdf)
        try:
            if to == "txt":
                text = "\n\n".join(doc[n].get_textpage().get_text_range() for n in range(len(doc)))
                return [self._write_text(ctx, f"{title}.txt", text.replace("\r\n", "\n"))]
            dpi = int(params.get("dpi") or 150)
            quality = int(params.get("quality") or 85)
            outs = []
            for n in range(len(doc)):
                ctx.check_cancelled()
                img = doc[n].render(scale=dpi / 72).to_pil().convert("RGB")
                name = f"{title}.{to}" if len(doc) == 1 else f"{title}-page-{n + 1}.{to}"
                out = ctx.output_path(name)
                buf = io.BytesIO()
                img.save(buf, PIL_FORMAT[to], **({"quality": quality} if to != "png" else {}))
                out.write_bytes(buf.getvalue())
                outs.append(OutputFile(out))
            return outs
        finally:
            doc.close()

    def _plain_pdf(self, ctx: JobContext, path: Path, check_only: bool = False) -> Path:
        """The PDF without its encryption: LibreOffice can't be given a
        password when it runs headless."""
        import pikepdf  # noqa: PLC0415

        password = (
            ctx.secrets.password.get_secret_value() if ctx.secrets and ctx.secrets.password else ""
        )
        try:
            with pikepdf.open(path, password=password) as pdf:
                if not pdf.is_encrypted or check_only:
                    return path
                out = ctx.temp_path(".pdf")
                pdf.save(out)  # without `encryption`, pikepdf writes it unencrypted
                return out
        except pikepdf.PasswordError:
            raise AdapterError("E_PASSWORD_WRONG" if password else "E_PASSWORD_REQUIRED") from None
        except pikepdf.PdfError:
            raise AdapterError("E_INPUT_CORRUPT") from None

    def _write_text(self, ctx: JobContext, name: str, text: str) -> OutputFile:
        out = ctx.output_path(name)
        out.write_text(text, "utf-8", newline="\n")
        return OutputFile(out)


def _sheet_name(produced: Path, stem: str) -> str:
    """LibreOffice names each sheet's CSV ``<stem>-<sheet>.csv``."""
    name = produced.stem
    sheet = name[len(stem) + 1 :] if name.startswith(stem + "-") else ""
    return re.sub(r"[^\w .()-]", "_", sheet).strip()


if __name__ == "__main__":
    run_main(OfficeConvertAdapter)
