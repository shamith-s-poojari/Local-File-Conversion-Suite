"""Shared table handling for the data adapters (ADR 0003, 0004): reading
CSV and Excel into rows, and writing rows back, under the same limits.

- CSV cells that are plainly numbers become numbers; anything that could
  lose information as a number (``007``, 16+ digits, ``1e5``) stays text.
- Text that looks like a formula is stored in Excel as text, never a live
  formula. Excel inputs are read for their stored values.

Imported only by adapters that require openpyxl (for Excel); the SDK
itself doesn't."""

from __future__ import annotations

import csv
import io
import re
import zipfile
from collections.abc import Callable
from pathlib import Path
from typing import Any

from .context import JobContext
from .convert import source_format
from .errors import AdapterError
from .markup import read_text

MB = 1024 * 1024
MAX_TEXT_BYTES = 64 * MB
MAX_XLSX_UNPACKED = 512 * MB
"""A workbook's XML, unpacked: past this it's a ZIP bomb or too big to hold."""
MAX_CELLS = 5_000_000
EXCEL_ROWS = 1_048_576
EXCEL_COLS = 16_384
EXCEL_CELL_CHARS = 32_767
INTEGER = re.compile(r"-?(0|[1-9][0-9]{0,14})")
DECIMAL = re.compile(r"-?(0|[1-9][0-9]{0,14})\.[0-9]{1,15}")
ILLEGAL_XML = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")
SHEET_NAME_BAD = re.compile(r"[\[\]:*?/\\]")
FILE_NAME_BAD = re.compile(r'[<>:"/\\|?*\x00-\x1f]')
DELIMITERS = {"comma": ",", "semicolon": ";", "tab": "\t"}


class Table:
    """Rows of cells, with the sheet name it came from or goes to. ``text``
    means the cells are CSV text, typed only where a format has numbers."""

    def __init__(self, name: str, rows: list[list[Any]], text: bool = False) -> None:
        self.name = name
        self.rows = rows
        self.text = text

    def typed(self) -> list[list[Any]]:
        return [[typed_cell(c) for c in r] for r in self.rows] if self.text else self.rows


# ---------------------------------------------------------------- values


def typed_cell(cell: Any) -> Any:
    """A CSV cell as a number when that loses nothing, else the text."""
    if not isinstance(cell, str):
        return cell
    if INTEGER.fullmatch(cell):
        return int(cell)
    if DECIMAL.fullmatch(cell):
        return float(cell)
    return cell


def from_excel(value: Any) -> Any:
    if isinstance(value, float) and value.is_integer() and abs(value) < 2**53:
        return int(value)
    return value


def trim(rows: list[list[Any]]) -> list[list[Any]]:
    """Drop trailing empty cells and rows (Excel reports its used range)."""
    out = []
    for row in rows:
        r = list(row)
        while r and (r[-1] is None or r[-1] == ""):
            r.pop()
        out.append(r)
    while out and not out[-1]:
        out.pop()
    return out


def csv_text(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float) and value.is_integer() and abs(value) < 2**53:
        return str(int(value))
    return str(value)


def excel_value(value: Any) -> Any:
    if value is None or isinstance(value, bool | int | float):
        if isinstance(value, int) and not isinstance(value, bool) and abs(value) >= 2**53:
            return str(value)  # Excel would round it
        return value
    text = ILLEGAL_XML.sub("", str(value))
    if len(text) > EXCEL_CELL_CHARS:
        raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "cellLength", "max": EXCEL_CELL_CHARS})
    return text


def sheet_name(name: str) -> str:
    clean = SHEET_NAME_BAD.sub("_", name).strip("'").strip() or "Sheet"
    return clean[:31]


def file_name(name: str) -> str:
    return FILE_NAME_BAD.sub("_", name).strip(" .") or "data"


def unique(name: str, used: set[str], limit: int = 255) -> str:
    candidate, n = name[:limit], 2
    while candidate.lower() in used:
        suffix = f" ({n})"
        candidate, n = name[: limit - len(suffix)] + suffix, n + 1
    used.add(candidate.lower())
    return candidate


# --------------------------------------------------------------- reading


def source_size(path: Path) -> int:
    """Bytes a source takes to hold (unpacked for Excel), within the limits."""
    size = path.stat().st_size
    if size == 0:
        raise AdapterError("E_INPUT_CORRUPT", {"reason": "empty"})
    if source_format(path) == "xlsx":
        unpacked = xlsx_unpacked_size(path)
        if unpacked > MAX_XLSX_UNPACKED:
            raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "bytes", "max": MAX_XLSX_UNPACKED})
        return unpacked
    if size > MAX_TEXT_BYTES:
        raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "bytes", "max": MAX_TEXT_BYTES})
    return size


def read_tables(ctx: JobContext, path: Path, warn: Callable[[str], None]) -> list[Table]:
    """A CSV file as one table, or an Excel workbook as one per sheet."""
    if source_format(path) == "csv":
        return [read_csv(ctx, path)]
    return read_xlsx(ctx, path, warn)


def read_csv(ctx: JobContext, path: Path) -> Table:
    text = read_text(path)
    if not text.strip():
        raise AdapterError("E_INPUT_CORRUPT", {"reason": "empty"})
    try:
        dialect: Any = csv.Sniffer().sniff(text[:65536], delimiters=",;\t|")
    except csv.Error:
        dialect = csv.excel
    rows = []
    cells = 0
    try:
        for row in csv.reader(io.StringIO(text), dialect):
            rows.append(row)
            cells += len(row)
            if cells > MAX_CELLS:
                raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "cells", "max": MAX_CELLS})
            if len(rows) % 5000 == 0:
                ctx.check_cancelled()
    except csv.Error:
        raise AdapterError("E_INPUT_CORRUPT") from None
    return Table(path.stem, trim(rows), text=True)


def read_xlsx(ctx: JobContext, path: Path, warn: Callable[[str], None]) -> list[Table]:
    import openpyxl  # noqa: PLC0415 - engine imported on demand

    if has_formulas(path):
        warn("FORMULAS_DROPPED")
    try:
        wb = openpyxl.load_workbook(path, read_only=True, data_only=True)
    except Exception:  # noqa: BLE001 - openpyxl raises many types for bad files
        raise AdapterError("E_INPUT_CORRUPT") from None
    tables = []
    cells = 0
    try:
        for ws in wb.worksheets:
            rows = []
            for row in ws.iter_rows(values_only=True):
                ctx.check_cancelled()
                cells += len(row)
                if cells > MAX_CELLS:
                    raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "cells", "max": MAX_CELLS})
                rows.append([from_excel(v) for v in row])
            tables.append(Table(ws.title, trim(rows)))
    except AdapterError:
        raise
    except Exception:  # noqa: BLE001
        raise AdapterError("E_INPUT_CORRUPT") from None
    finally:
        wb.close()
    tables = [t for t in tables if t.rows] or tables[:1]
    if not tables:
        raise AdapterError("E_INPUT_CORRUPT", {"reason": "empty"})
    return tables


def xlsx_unpacked_size(path: Path) -> int:
    try:
        with zipfile.ZipFile(path) as z:
            return sum(i.file_size for i in z.infolist())
    except (zipfile.BadZipFile, OSError):
        raise AdapterError("E_INPUT_CORRUPT") from None


def has_formulas(path: Path) -> bool:
    """Whether any sheet has a formula (an ``<f>`` element), read in chunks."""
    try:
        with zipfile.ZipFile(path) as z:
            for name in z.namelist():
                if not (name.startswith("xl/worksheets/") and name.endswith(".xml")):
                    continue
                with z.open(name) as f:
                    tail = b""
                    while chunk := f.read(1 << 20):
                        data = tail + chunk
                        if b"<f>" in data or b"<f " in data:
                            return True
                        tail = data[-3:]
    except (zipfile.BadZipFile, OSError):
        raise AdapterError("E_INPUT_CORRUPT") from None
    return False


# --------------------------------------------------------------- writing


def write_csv(out: Path, rows: list[list[Any]], delimiter: str = ",") -> None:
    buf = io.StringIO()
    w = csv.writer(buf, delimiter=delimiter, lineterminator="\n")
    for row in rows:
        w.writerow([csv_text(c) for c in row])
    out.write_text(buf.getvalue(), "utf-8", newline="")


def write_xlsx(ctx: JobContext, out: Path, tables: list[Table], stage: str = "convert") -> None:
    import openpyxl  # noqa: PLC0415
    from openpyxl.cell import WriteOnlyCell  # noqa: PLC0415

    wb = openpyxl.Workbook(write_only=True)
    used: set[str] = set()
    total = sum(len(t.rows) for t in tables)
    done = 0
    for t in tables:
        if len(t.rows) > EXCEL_ROWS or any(len(r) > EXCEL_COLS for r in t.rows):
            raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "excelSize", "max": EXCEL_ROWS})
        ws = wb.create_sheet(unique(sheet_name(t.name), used, limit=31))
        for r in t.typed():
            done += 1
            if done % 2000 == 0:
                ctx.check_cancelled()
                ctx.progress(stage, done, total)
            cells = []
            for value in r:
                cell = WriteOnlyCell(ws, value=excel_value(value))
                if isinstance(cell.value, str):
                    # Never a live formula, whatever the text looks like.
                    cell.data_type = "s"
                cells.append(cell)
            ws.append(cells)
    if not tables:
        wb.create_sheet("Sheet1")
    wb.properties.creator = None
    buf = io.BytesIO()
    wb.save(buf)
    out.write_bytes(buf.getvalue())


__all__ = [
    "DELIMITERS",
    "MAX_CELLS",
    "Table",
    "csv_text",
    "file_name",
    "read_csv",
    "read_tables",
    "read_xlsx",
    "source_size",
    "trim",
    "typed_cell",
    "unique",
    "write_csv",
    "write_xlsx",
]
