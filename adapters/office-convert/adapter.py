"""office-convert: Word, PowerPoint, Excel, OpenDocument, RTF, CSV, plain
text, HTML and Markdown through LibreOffice (ADR 0003).

Every conversion is one job here; the ones that need a step in between
(a document to PNG goes through PDF, Markdown to Word through HTML) do it
internally, in ``work/``.
"""

from __future__ import annotations

import html
import io
import re
import shutil
import struct
import zipfile
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
from converter_sdk.limits import MB, check_pixels, rendered_size
from converter_sdk.markup import html_to_markdown, markdown_to_html, read_text
from converter_sdk.office import (
    IMPORT_FILTERS,
    export_filter,
    is_encrypted_ooxml,
    reject_encrypted_ooxml,
    soffice_convert,
)
from converter_sdk.protocol import ValidationIssue

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
            rel = p.relative_to(ctx.job_dir.resolve()).as_posix()
            if is_encrypted_ooxml(p, source_format(p)):
                # Not corrupt: Office wrapped it for a password LibreOffice can't be given.
                report.errors.append(
                    ValidationIssue(
                        code="E_PASSWORD_REQUIRED", input=rel, detail={"reason": "officePassword"}
                    )
                )
                continue
            if source_format(p) == "pdf":
                try:
                    self._plain_pdf(ctx, p, check_only=True)
                except AdapterError as e:
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
            reject_encrypted_ooxml(path, source_format(path))
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

        # Not disarmed first (audit P15): that would drop objects and links the
        # conversion should keep; see "Why inputs aren't disarmed" in the README.
        if to == "txt" and src in CALC:
            produced = soffice_convert(ctx, source, TEXT_SHEETS, import_filter=import_filter)
            produced = _in_sheet_order(produced, path)
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
            produced = _in_sheet_order(produced, path)
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
                width, height = doc[n].get_size()
                check_pixels(*rendered_size(width, height, dpi / 72))
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
                # Whatever is made from this copy has no password or limits.
                ctx.warn_encryption_removed()
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


_XLSX_SHEET = re.compile(rb"<(?:\w+:)?sheet\b[^>]*?\bname=\"([^\"]*)\"")
_ODS_SHEET = re.compile(rb"<table:table\b[^>]*?\btable:name=\"([^\"]*)\"")


def _workbook_sheets(path: Path) -> list[str]:
    """Sheet names in workbook order, read from an xlsx, ods or xls; ``[]``
    when the file is another kind or can't be read (the caller keeps its order)."""
    try:
        with path.open("rb") as f:
            is_compound = f.read(8) == _CFB_MAGIC
    except OSError:
        return []
    if is_compound:
        return _xls_sheets(path)
    try:
        with zipfile.ZipFile(path) as z:
            names = set(z.namelist())
            if "xl/workbook.xml" in names:
                found = _XLSX_SHEET.findall(z.read("xl/workbook.xml"))
            elif "content.xml" in names:
                found = _ODS_SHEET.findall(z.read("content.xml"))
            else:
                return []
    except (OSError, zipfile.BadZipFile, KeyError):
        return []
    return [html.unescape(n.decode("utf-8", "replace")) for n in found]


_CFB_MAGIC = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
_CFB_LAST = 0xFFFFFFFA
"""Sector numbers from here up are markers (end of chain, free), not sectors."""


def _cfb_stream(data: bytes, name: str) -> bytes:
    """One stream of a compound file (the container of an .xls), followed
    through its sector chain, or through the mini stream for a stream under
    the cutoff (4096 bytes). Raises ValueError on anything malformed."""
    if len(data) < 512 or data[:8] != _CFB_MAGIC:
        raise ValueError("notCompound")
    shift = struct.unpack_from("<H", data, 0x1E)[0]
    if shift not in (9, 12):
        raise ValueError("sectorSize")
    size = 1 << shift
    per = size // 4
    limit = len(data) // size + 1
    """No chain can be longer than the file has sectors, so a loop stops here."""

    def sector(n: int) -> bytes:
        off = (n + 1) * size
        if n >= _CFB_LAST or off + size > len(data):
            raise ValueError("sector")
        return data[off : off + size]

    fat_count, dir_start = struct.unpack_from("<II", data, 0x2C)
    cutoff = struct.unpack_from("<I", data, 0x38)[0]
    difat = list(struct.unpack_from("<109I", data, 0x4C))
    nxt = struct.unpack_from("<I", data, 0x44)[0]
    for _ in range(limit):
        if nxt >= _CFB_LAST:
            break
        entries = struct.unpack(f"<{per}I", sector(nxt))
        difat += entries[:-1]
        nxt = entries[-1]
    else:
        raise ValueError("difatLoop")
    if fat_count > len(difat):
        raise ValueError("fatCount")
    fat: list[int] = []
    for n in difat[:fat_count]:
        fat += struct.unpack(f"<{per}I", sector(n))

    def chain(start: int, table: list[int], read) -> bytes:
        parts, n = [], start
        while n < _CFB_LAST:
            if len(parts) >= len(table) or n >= len(table):
                raise ValueError("chain")
            parts.append(read(n))
            n = table[n]
        return b"".join(parts)

    directory = chain(dir_start, fat, sector)
    if len(directory) < 128 or directory[66] != 5:  # 5: the root entry
        raise ValueError("noRoot")
    for off in range(0, len(directory) - 127, 128):
        entry = directory[off : off + 128]
        name_len = struct.unpack_from("<H", entry, 64)[0]
        if entry[66] != 2 or not 2 <= name_len <= 64:  # 2: a stream
            continue
        if entry[: name_len - 2].decode("utf-16-le", "replace") != name:
            continue
        start, length = struct.unpack_from("<II", entry, 116)
        if length >= cutoff:
            stream = chain(start, fat, sector)
        else:
            stream = _cfb_mini(data, directory, fat, sector, chain)(start)
        if len(stream) < length:
            raise ValueError("short")
        return stream[:length]
    raise ValueError("noStream")


def _cfb_mini(data: bytes, directory: bytes, fat: list[int], sector, chain):
    """A reader for the mini stream: small streams kept in 64-byte pieces
    inside the root entry's own stream, chained by the mini FAT."""
    mini_size = 1 << struct.unpack_from("<H", data, 0x20)[0]
    minifat_start = struct.unpack_from("<I", data, 0x3C)[0]
    minifat_raw = chain(minifat_start, fat, sector) if minifat_start < _CFB_LAST else b""
    minifat = list(struct.unpack(f"<{len(minifat_raw) // 4}I", minifat_raw))
    container = chain(struct.unpack_from("<I", directory, 116)[0], fat, sector)

    def piece(n: int) -> bytes:
        if (n + 1) * mini_size > len(container):
            raise ValueError("miniSector")
        return container[n * mini_size : (n + 1) * mini_size]

    return lambda start: chain(start, minifat, piece)


_BIFF_BOF, _BIFF_EOF, _BIFF_SHEET = 0x0809, 0x000A, 0x0085


def _xls_sheets(path: Path) -> list[str]:
    """Sheet names in order from an Excel 97 (BIFF8) workbook: the sheet
    records of its first part, which ends at the first EOF record."""
    try:
        stream = _cfb_stream(path.read_bytes(), "Workbook")
        kind, size = struct.unpack_from("<HH", stream, 0)
        if kind != _BIFF_BOF or size < 2 or struct.unpack_from("<H", stream, 4)[0] != 0x0600:
            return []
        names, pos = [], 0
        while pos + 4 <= len(stream):
            kind, size = struct.unpack_from("<HH", stream, pos)
            body = stream[pos + 4 : pos + 4 + size]
            pos += 4 + size
            if kind == _BIFF_EOF:
                return names
            if kind == _BIFF_SHEET and len(body) >= 8:
                count, wide = body[6], body[7] & 1
                chars = body[8 : 8 + count * (2 if wide else 1)]
                names.append(chars.decode("utf-16-le" if wide else "latin-1"))
    except (OSError, ValueError, struct.error):
        return []
    return []


def _in_sheet_order(produced: list[Path], source: Path) -> list[Path]:
    """One file per sheet, in the workbook's own order. LibreOffice's write
    order (file times) can't be trusted for that: on Windows the files of one
    run can share a timestamp."""
    if len(produced) < 2:
        return produced
    clean = [re.sub(r"[^\w .()-]", "_", n).strip() for n in _workbook_sheets(source)]
    rank = {name: i for i, name in enumerate(clean)}
    return sorted(produced, key=lambda p: rank.get(_sheet_name(p, source.stem), len(rank)))


if __name__ == "__main__":
    run_main(OfficeConvertAdapter)
