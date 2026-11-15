"""pdf-to-document: PDF to Word, Excel, Markdown, HTML and plain text (ADR 0003).

- Word goes through pdf2docx, which rebuilds paragraphs, tables and images
  from the page layout.
- Excel gets the tables PyMuPDF finds, one sheet per table. A PDF with no
  tables gives one sheet of its text lines.
- Markdown and HTML rebuild headings (from font sizes), paragraphs, bold
  and italic, and tables. Images are left out (``IMAGES_DROPPED``).
- Plain text is the text layer in reading order.

Text that isn't in the PDF can't come out: a scan without a text layer gives
``E_NO_TEXT`` for the text formats and ``NO_TEXT_LAYER`` for Word.
"""

from __future__ import annotations

import html as html_lib
import io
import logging
import re
from collections import Counter
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import AdapterError, JobContext, OutputFile, ValidationReport, run_main
from converter_sdk.convert import Tier, source_format
from converter_sdk.errors import Cancelled
from converter_sdk.markup import escape_markdown, html_document
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
TARGETS: dict[str, Tier] = {"docx": "R", "xlsx": "R", "md": "R", "html": "R", "txt": "R"}
EDGES: dict[tuple[str, str], Tier] = {("pdf", to): tier for to, tier in TARGETS.items()}
DOCX_CHUNK = 10
"""Pages pdf2docx analyses at a time. Its whole-document pass can't be
cancelled, so it runs on a few pages at once; it also caps memory."""
HEADING_RATIO = 1.18
"""Text this much larger than the body text is a heading."""
NUMBER = re.compile(r"-?\d{1,15}(\.\d{1,10})?")


@dataclass
class Run:
    text: str
    bold: bool = False
    italic: bool = False


@dataclass
class Block:
    kind: Literal["h", "p", "table"]
    runs: list[Run] | None = None
    level: int = 0
    rows: list[list[str]] | None = None


class PdfToDocumentAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-to-document"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "R"
    EDGES = EDGES
    base_ram_bytes: ClassVar[int] = 192 * MB
    """Interpreter, PyMuPDF, and pdf2docx's OpenCV and NumPy."""
    ram_per_input_byte: ClassVar[float] = 6.0
    disk_factor: ClassVar[float] = 4.0

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        to = str(params.get("to", ""))
        if to not in TARGETS or any(source_format(o.path) != "pdf" for o in inputs):
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
        # Reading a PDF changes nothing in it: signature, form and
        # active-content warnings don't apply.
        report.warnings = []
        return report

    # execute ----------------------------------------------------------

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = str(params["to"])
        ranges = str(params.get("pages", "") or "")
        plans = []
        for path in ctx.inputs:
            doc = self._open(ctx, path)
            count = len(doc)
            doc.close()
            pages = (
                list(pages_in(parse_page_ranges(ranges, count)))
                if ranges.strip()
                else list(range(1, count + 1))
            )
            plans.append((path, pages))
        steps = 2 if to == "docx" else 1
        self._total = sum(len(p) for _, p in plans) * steps
        self._done = 0
        self._warned: set[str] = set()
        ctx.stage("convert", 1, 1)
        outputs = []
        for index, (path, pages) in enumerate(plans):
            title = _title(params, index)
            out = ctx.output_path(f"{title}.{to}")
            if to == "docx":
                self._docx(ctx, path, pages, out)
            else:
                doc = self._open(ctx, path)
                try:
                    {
                        "xlsx": self._xlsx,
                        "md": self._markdown,
                        "html": self._html,
                        "txt": self._text,
                    }[to](ctx, doc, pages, out, title)
                finally:
                    doc.close()
            outputs.append(OutputFile(out, source=index))
        ctx.progress("convert", self._total, self._total)
        return outputs

    # targets ----------------------------------------------------------

    def _docx(self, ctx: JobContext, path: Path, pages: list[int], out: Path) -> None:
        from docx import Document  # noqa: PLC0415 - python-docx, with pdf2docx
        from pdf2docx import Converter  # noqa: PLC0415

        document = Document()
        made = 0
        failed = False
        text = False
        repaired = False
        with _quiet_logging():
            for start in range(0, len(pages), DOCX_CHUNK):
                chunk = pages[start : start + DOCX_CHUNK]
                ctx.check_cancelled()
                cv = Converter(str(path), password=self._password(ctx))
                try:
                    settings = cv.default_settings
                    cv.load_pages(pages=[n - 1 for n in chunk])
                    text = text or any(cv.fitz_doc[n - 1].get_text("text").strip() for n in chunk)
                    repaired = repaired or cv.fitz_doc.is_repaired
                    cv.parse_document(**settings)
                    for page in [p for p in cv.pages if not p.skip_parsing]:
                        ctx.check_cancelled()
                        try:
                            page.parse(**settings)
                        except Cancelled:
                            raise
                        except Exception:  # noqa: BLE001 - one bad page shouldn't sink the rest
                            failed = True
                        self._step(ctx)
                    for page in [p for p in cv.pages if p.finalized]:
                        ctx.check_cancelled()
                        try:
                            page.make_docx(document)
                            made += 1
                        except Cancelled:
                            raise
                        except Exception:  # noqa: BLE001
                            failed = True
                        self._step(ctx)
                except Cancelled:
                    raise
                except AdapterError:
                    raise
                except Exception:  # noqa: BLE001 - pdf2docx raises plain exceptions for unreadable PDFs
                    raise AdapterError("E_INPUT_CORRUPT") from None
                finally:
                    cv.close()
        if made == 0:
            raise AdapterError("E_INPUT_CORRUPT", {"reason": "noPages"})
        if not text and repaired:
            raise AdapterError("E_INPUT_CORRUPT")
        if not text:
            self._warn(ctx, "NO_TEXT_LAYER")
        if failed:
            self._warn(ctx, "LAYOUT_APPROXIMATED")
        buf = io.BytesIO()
        document.save(buf)
        out.write_bytes(buf.getvalue())

    def _xlsx(self, ctx: JobContext, doc: Any, pages: list[int], out: Path, title: str) -> None:
        import openpyxl  # noqa: PLC0415

        wb = openpyxl.Workbook()
        wb.remove(wb.active)
        lines: list[str] = []
        for n in pages:
            ctx.check_cancelled()
            page = doc[n - 1]
            tables = [
                t for t in page.find_tables().tables if any(any(c for c in r) for r in t.extract())
            ]
            for k, table in enumerate(tables, start=1):
                name = f"Page {n}" if len(tables) == 1 else f"Page {n} ({k})"
                _append_rows(wb.create_sheet(name[:31]), table.extract())
            lines += [
                line for line in page.get_text("text", sort=True).splitlines() if line.strip()
            ]
            self._step(ctx)
        if not lines and not wb.worksheets:
            _no_text(doc)
        if not wb.worksheets:
            # No tables: the text, a line per row, is better than nothing.
            _append_rows(wb.create_sheet("Text"), [[line] for line in lines])
            self._warn(ctx, "LAYOUT_APPROXIMATED")
        wb.properties.creator = None
        wb.properties.lastModifiedBy = None
        buf = io.BytesIO()
        wb.save(buf)
        out.write_bytes(buf.getvalue())

    def _text(self, ctx: JobContext, doc: Any, pages: list[int], out: Path, title: str) -> None:
        parts = []
        for n in pages:
            ctx.check_cancelled()
            text = doc[n - 1].get_text("text", sort=True)
            parts.append("\n".join(line.rstrip() for line in text.splitlines()).strip("\n"))
            self._step(ctx)
        text = "\n\n".join(p for p in parts if p.strip())
        if not text.strip():
            _no_text(doc)
        if any(not p.strip() for p in parts):
            self._warn(ctx, "NO_TEXT_LAYER")
        out.write_text(text + "\n", "utf-8", newline="\n")

    def _markdown(self, ctx: JobContext, doc: Any, pages: list[int], out: Path, title: str) -> None:
        blocks = self._structure(ctx, doc, pages)
        parts = []
        for b in blocks:
            if b.kind == "h":
                parts.append(f"{'#' * b.level} {_md_runs(b.runs or [], plain=True)}")
            elif b.kind == "p":
                parts.append(_md_runs(b.runs or []))
            else:
                parts.append(_md_table(b.rows or []))
        out.write_text("\n\n".join(p for p in parts if p.strip()) + "\n", "utf-8", newline="\n")

    def _html(self, ctx: JobContext, doc: Any, pages: list[int], out: Path, title: str) -> None:
        blocks = self._structure(ctx, doc, pages)
        parts = []
        for b in blocks:
            if b.kind == "h":
                parts.append(f"<h{b.level}>{_html_runs(b.runs or [], plain=True)}</h{b.level}>")
            elif b.kind == "p":
                parts.append(f"<p>{_html_runs(b.runs or [])}</p>")
            else:
                parts.append(_html_table(b.rows or []))
        out.write_text(html_document("\n".join(parts), title), "utf-8", newline="\n")

    # layout -----------------------------------------------------------

    def _structure(self, ctx: JobContext, doc: Any, pages: list[int]) -> list[Block]:
        """Headings, paragraphs and tables, in reading order."""
        import pymupdf  # noqa: PLC0415

        sizes: Counter[float] = Counter()
        read = []
        images = False
        for n in pages:
            ctx.check_cancelled()
            page = doc[n - 1]
            tables = page.find_tables().tables
            data = page.get_text("dict", sort=True, flags=pymupdf.TEXTFLAGS_TEXT)
            images = images or bool(page.get_images())
            for b in data["blocks"]:
                for line in b.get("lines", []):
                    for s in line["spans"]:
                        sizes[round(s["size"], 1)] += len(s["text"].strip())
            read.append((data, tables))
            self._step(ctx)
        if not sizes or sum(sizes.values()) == 0:
            _no_text(doc)
        body = max(sizes.items(), key=lambda kv: (kv[1], -kv[0]))[0]
        bigger = sorted((s for s in sizes if s >= body * HEADING_RATIO), reverse=True)
        level = {s: min(i + 1, 3) for i, s in enumerate(bigger)}
        if images:
            self._warn(ctx, "IMAGES_DROPPED")

        blocks: list[Block] = []
        for data, tables in read:
            boxes = [pymupdf.Rect(t.bbox) for t in tables]
            placed: list[tuple[float, Block]] = []
            for t in tables:
                rows = [[_cell_text(c) for c in row] for row in t.extract()]
                if any(any(c for c in r) for r in rows):
                    placed.append((t.bbox[1], Block("table", rows=rows)))
            for b in data["blocks"]:
                rect = pymupdf.Rect(b["bbox"])
                if any((rect & box).get_area() > 0.5 * max(rect.get_area(), 1e-6) for box in boxes):
                    continue
                runs, size = _block_runs(b)
                text = "".join(r.text for r in runs).strip()
                if not text:
                    continue
                lvl = level.get(round(size, 1), 0)
                if lvl and len(text) <= 200:
                    placed.append((b["bbox"][1], Block("h", runs=runs, level=lvl)))
                else:
                    placed.append((b["bbox"][1], Block("p", runs=runs)))
            placed.sort(key=lambda item: item[0])
            blocks += [blk for _, blk in placed]
        return blocks

    # helpers ----------------------------------------------------------

    def _step(self, ctx: JobContext) -> None:
        self._done += 1
        ctx.progress("convert", min(self._done, self._total), self._total)

    def _warn(self, ctx: JobContext, code: str) -> None:
        if code not in self._warned:
            self._warned.add(code)
            ctx.warn(code)

    def _password(self, ctx: JobContext) -> str | None:
        if ctx.secrets is not None and ctx.secrets.password is not None:
            return ctx.secrets.password.get_secret_value()
        return None

    def _open(self, ctx: JobContext, path: Path) -> Any:
        import pymupdf  # noqa: PLC0415

        try:
            doc = pymupdf.open(path, filetype="pdf")
        except Exception:  # noqa: BLE001 - PyMuPDF raises several types for damaged files
            raise AdapterError("E_INPUT_CORRUPT") from None
        if doc.needs_pass:
            password = self._password(ctx)
            if not password:
                doc.close()
                raise AdapterError("E_PASSWORD_REQUIRED")
            if not doc.authenticate(password):
                doc.close()
                raise AdapterError("E_PASSWORD_WRONG")
        return doc


# ------------------------------------------------------------------ text


def _no_text(doc: Any) -> None:
    """No text at all: a scan (``E_NO_TEXT``), unless MuPDF had to repair
    the file to open it, when the text is more likely lost to damage."""
    raise AdapterError("E_INPUT_CORRUPT" if doc.is_repaired else "E_NO_TEXT")


def _title(params: dict[str, Any], index: int) -> str:
    titles = params.get("fileTitles") or []
    if index < len(titles) and isinstance(titles[index], str) and titles[index].strip():
        return titles[index].strip()
    return f"document-{index + 1}"


@contextmanager
def _quiet_logging() -> Iterator[None]:
    """pdf2docx logs page text in its error messages; none of it may reach stderr."""
    logging.disable(logging.CRITICAL)
    try:
        yield
    finally:
        logging.disable(logging.NOTSET)


def _block_runs(block: dict[str, Any]) -> tuple[list[Run], float]:
    """A text block's styled runs, lines joined, and its largest font size."""
    runs: list[Run] = []
    size = 0.0
    for i, line in enumerate(block.get("lines", [])):
        if i and runs:
            prev = runs[-1].text
            if (
                re.search(r"[a-z]-$", prev)
                and line["spans"]
                and line["spans"][0]["text"][:1].islower()
            ):
                runs[-1].text = prev[:-1]
            elif not prev.endswith(" "):
                runs.append(Run(" "))
        for s in line["spans"]:
            if not s["text"]:
                continue
            if s["text"].strip():
                size = max(size, float(s["size"]))
            font = str(s.get("font", "")).lower()
            bold = bool(s["flags"] & 16) or "bold" in font or "black" in font
            italic = bool(s["flags"] & 2) or "italic" in font or "oblique" in font
            if runs and runs[-1].bold == bold and runs[-1].italic == italic:
                runs[-1].text += s["text"]
            elif runs and not s["text"].strip():
                runs[-1].text += s["text"]
            else:
                runs.append(Run(s["text"], bold, italic))
    return runs, size


def _wrap(text: str, before: str, after: str) -> str:
    """Markers around the words, spaces outside them (``**a** b``, not ``** a**``)."""
    core = text.strip()
    if not core:
        return text
    lead = text[: len(text) - len(text.lstrip())]
    trail = text[len(text.rstrip()) :]
    return f"{lead}{before}{core}{after}{trail}"


def _md_runs(runs: list[Run], plain: bool = False) -> str:
    out = []
    every_bold = all(r.bold for r in runs if r.text.strip())
    for r in runs:
        text = escape_markdown(r.text)
        if plain:
            out.append(text)
            continue
        marker = ("**" if r.bold and not every_bold else "") + ("*" if r.italic else "")
        out.append(_wrap(text, marker, marker[::-1]) if marker else text)
    if every_bold and not plain and runs:
        return _wrap("".join(out), "**", "**")
    return re.sub(r"\s+", " ", "".join(out)).strip()


def _html_runs(runs: list[Run], plain: bool = False) -> str:
    out = []
    for r in runs:
        text = html_lib.escape(r.text)
        if not plain and r.text.strip():
            if r.italic:
                text = _wrap(text, "<em>", "</em>")
            if r.bold:
                text = _wrap(text, "<strong>", "</strong>")
        out.append(text)
    return re.sub(r"\s+", " ", "".join(out)).strip()


def _cell_text(cell: Any) -> str:
    return re.sub(r"\s+", " ", str(cell)).strip() if cell is not None else ""


def _md_table(rows: list[list[str]]) -> str:
    width = max(len(r) for r in rows)
    rows = [r + [""] * (width - len(r)) for r in rows]
    cells = [[escape_markdown(c) for c in r] for r in rows]
    lines = ["| " + " | ".join(cells[0]) + " |", "|" + "---|" * width]
    lines += ["| " + " | ".join(r) + " |" for r in cells[1:]]
    return "\n".join(lines)


def _html_table(rows: list[list[str]]) -> str:
    head = "".join(f"<th>{html_lib.escape(c)}</th>" for c in rows[0])
    body = "".join(
        "<tr>" + "".join(f"<td>{html_lib.escape(c)}</td>" for c in r) + "</tr>" for r in rows[1:]
    )
    return f"<table><thead><tr>{head}</tr></thead><tbody>{body}</tbody></table>"


def _append_rows(ws: Any, rows: list[list[Any]]) -> None:
    """Rows into a sheet: numbers as numbers, everything else as text. A cell
    like ``=HYPERLINK(…)`` from the PDF stays text, never a live formula."""
    for row in rows:
        values: list[Any] = []
        for cell in row:
            text = "" if cell is None else str(cell).strip()
            if NUMBER.fullmatch(text):
                values.append(float(text) if "." in text else int(text))
            else:
                values.append(text or None)
        ws.append(values)
        for c in ws[ws.max_row]:
            if isinstance(c.value, str):
                c.data_type = "s"


if __name__ == "__main__":
    run_main(PdfToDocumentAdapter)
