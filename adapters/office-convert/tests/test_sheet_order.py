"""Per-sheet outputs follow the workbook's order, not LibreOffice's file times
(on Windows the files of one run can share a timestamp). Needs no LibreOffice."""

from __future__ import annotations

import importlib.util
import os
import struct
import zipfile
from pathlib import Path

import pytest

_spec = importlib.util.spec_from_file_location(
    "office_convert_adapter", Path(__file__).parent.parent / "adapter.py"
)
assert _spec and _spec.loader
adapter = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(adapter)


def _produced(tmp_path: Path, stem: str, sheets: list[str]) -> list[Path]:
    """Files named as LibreOffice names them, all with the same time, made
    in the given order."""
    out = []
    for sheet in sheets:
        p = tmp_path / "lo-out" / f"{stem}-{sheet}.csv"
        p.parent.mkdir(exist_ok=True)
        p.write_text("x\n")
        os.utime(p, ns=(1_700_000_000_000_000_000, 1_700_000_000_000_000_000))
        out.append(p)
    return out


def _xlsx(path: Path, sheets: list[str]) -> Path:
    entries = "".join(
        f'<sheet name="{n}" sheetId="{i + 1}" r:id="rId{i + 1}"/>' for i, n in enumerate(sheets)
    )
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("xl/workbook.xml", f"<workbook><sheets>{entries}</sheets></workbook>")
    return path


def _ods(path: Path, sheets: list[str]) -> Path:
    tables = "".join(f'<table:table table:name="{n}"></table:table>' for n in sheets)
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("content.xml", f"<office:document-content>{tables}</office:document-content>")
    return path


_END, _FREE = 0xFFFFFFFE, 0xFFFFFFFF


def _biff8(sheets: list[str]) -> bytes:
    """A workbook stream's first part: BOF, one sheet record per sheet
    (Latin-1 and UTF-16 names both), EOF."""
    out = struct.pack("<HHHH", 0x0809, 4, 0x0600, 0x0005)
    for i, name in enumerate(sheets):
        wide = i % 2
        chars = name.encode("utf-16-le" if wide else "latin-1")
        body = struct.pack("<IHBB", 0, 0, len(name), wide) + chars
        out += struct.pack("<HH", 0x0085, len(body)) + body
    return out + struct.pack("<HH", 0x000A, 0)


def _entry(name: str, kind: int, start: int, size: int) -> bytes:
    raw = name.encode("utf-16-le") + b"\0\0"
    e = bytearray(128)
    e[: len(raw)] = raw
    struct.pack_into("<HBB", e, 64, len(raw), kind, 1)
    struct.pack_into("<III", e, 68, _FREE, _FREE, _FREE)
    struct.pack_into("<II", e, 116, start, size)
    return bytes(e)


def _xls(path: Path, sheets: list[str], mini: bool = False) -> Path:
    """A compound file (512-byte sectors) holding one Workbook stream, kept
    in the mini stream when ``mini`` and in plain sectors otherwise."""
    stream = _biff8(sheets)
    fat = [0xFFFFFFFD, _END]  # sector 0: the FAT; 1: the directory
    if mini:
        container = stream + bytes(-len(stream) % 64)
        pieces = len(container) // 64
        minifat = [*range(1, pieces), _END]
        minifat += [_FREE] * (128 - len(minifat))
        body_sectors = -(-len(container) // 512)
        fat += [_END]  # sector 2: the mini FAT
        fat += [*range(4, 3 + body_sectors), _END]
        root = _entry("Root Entry", 5, 3, len(container))
        book = _entry("Workbook", 2, 0, len(stream))
        sectors = struct.pack("<128I", *minifat) + container + bytes(-len(container) % 512)
        minifat_start, minifat_count = 2, 1
    else:
        stream += bytes(4096 - len(stream))
        fat += [*range(3, 2 + len(stream) // 512), _END]
        root = _entry("Root Entry", 5, _END, 0)
        book = _entry("Workbook", 2, 2, len(stream))
        sectors = stream
        minifat_start, minifat_count = _END, 0
    fat += [_FREE] * (128 - len(fat))
    header = bytearray(512)
    header[:8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
    struct.pack_into("<HHHHH", header, 0x18, 0x3E, 3, 0xFFFE, 9, 6)
    struct.pack_into("<III", header, 0x2C, 1, 1, 0)
    struct.pack_into("<IIIII", header, 0x38, 4096, minifat_start, minifat_count, _END, 0)
    struct.pack_into("<109I", header, 0x4C, 0, *[_FREE] * 108)
    directory = root + book + bytes(256)
    path.write_bytes(bytes(header) + struct.pack("<128I", *fat) + directory + sectors)
    return path


def _xls_mini(path: Path, sheets: list[str]) -> Path:
    return _xls(path, sheets, mini=True)


@pytest.mark.parametrize("make", [_xlsx, _ods, _xls, _xls_mini])
def test_outputs_follow_the_workbook_order(tmp_path: Path, make) -> None:
    escaped = make in (_xlsx, _ods)  # XML escapes the ampersand; BIFF doesn't
    sheets = ["Sales", "Notes", "R&amp;D" if escaped else "R&D"]
    source = make(tmp_path / f"report.{make.__name__[1:4]}", sheets)
    produced = _produced(tmp_path, "report", ["Notes", "R&D", "Sales"])
    ordered = adapter._in_sheet_order(produced, source)
    assert [p.stem for p in ordered] == ["report-Sales", "report-Notes", "report-R&D"]


def test_an_unreadable_workbook_keeps_the_order_given(tmp_path: Path) -> None:
    source = tmp_path / "report.xls"
    source.write_bytes(b"\xd0\xcf\x11\xe0 not a zip")
    produced = _produced(tmp_path, "report", ["Notes", "Sales"])
    assert adapter._in_sheet_order(produced, source) == produced


def test_a_compound_file_with_a_sector_loop_keeps_the_order_given(tmp_path: Path) -> None:
    source = _xls(tmp_path / "report.xls", ["Sales", "Notes"])
    data = bytearray(source.read_bytes())
    struct.pack_into("<I", data, 512 + 4 * 2, 2)  # the stream's first sector points to itself
    source.write_bytes(bytes(data))
    produced = _produced(tmp_path, "report", ["Notes", "Sales"])
    assert adapter._workbook_sheets(source) == []
    assert adapter._in_sheet_order(produced, source) == produced
