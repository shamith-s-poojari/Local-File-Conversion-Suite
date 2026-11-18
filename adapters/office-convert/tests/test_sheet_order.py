"""Per-sheet outputs follow the workbook's order, not LibreOffice's file times
(on Windows the files of one run can share a timestamp). Needs no LibreOffice."""

from __future__ import annotations

import importlib.util
import os
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


@pytest.mark.parametrize("make", [_xlsx, _ods])
def test_outputs_follow_the_workbook_order(tmp_path: Path, make) -> None:
    source = make(tmp_path / f"report.{make.__name__[1:]}", ["Sales", "Notes", "R&amp;D"])
    produced = _produced(tmp_path, "report", ["Notes", "R&D", "Sales"])
    ordered = adapter._in_sheet_order(produced, source)
    assert [p.stem for p in ordered] == ["report-Sales", "report-Notes", "report-R&D"]


def test_an_unreadable_workbook_keeps_the_order_given(tmp_path: Path) -> None:
    source = tmp_path / "report.xls"
    source.write_bytes(b"\xd0\xcf\x11\xe0 not a zip")
    produced = _produced(tmp_path, "report", ["Notes", "Sales"])
    assert adapter._in_sheet_order(produced, source) == produced
