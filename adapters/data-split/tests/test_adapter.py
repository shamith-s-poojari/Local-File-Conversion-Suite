"""Acceptance tests for data-split (ADR 0004)."""

from __future__ import annotations

import csv
from pathlib import Path

import openpyxl
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]


def _csv(path: Path, rows: list[list[str]]) -> Path:
    with path.open("w", newline="", encoding="utf-8") as f:
        csv.writer(f).writerows(rows)
    return path


def _read(path: Path) -> list[list[str]]:
    return list(csv.reader(path.read_text("utf-8").splitlines()))


def test_every_n_rows_repeats_the_header(tmp_path: Path) -> None:
    src = _csv(tmp_path / "in.csv", [["n"], *[[str(i)] for i in range(5)]])
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"rows": 2, "fileTitles": ["nums"]})
    assert not r.errors
    assert r.names == ["nums-part-1.csv", "nums-part-2.csv", "nums-part-3.csv"]
    assert r.sources == [0, 0, 0]
    assert [_read(o) for o in r.outputs] == [
        [["n"], ["0"], ["1"]],
        [["n"], ["2"], ["3"]],
        [["n"], ["4"]],
    ]


def test_by_column_value(tmp_path: Path) -> None:
    src = _csv(
        tmp_path / "in.csv",
        [["Name", "City"], ["Ada", "Oslo"], ["Bo", "Rome"], ["Cy", "Oslo"], ["Di", ""]],
    )
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [src],
        {"mode": "column", "column": " city ", "fileTitles": ["p"]},
    )
    assert r.names == ["p-Oslo.csv", "p-Rome.csv", "p-(empty).csv"]
    assert _read(r.outputs[0]) == [["Name", "City"], ["Ada", "Oslo"], ["Cy", "Oslo"]]


def test_unknown_column_is_an_error(tmp_path: Path) -> None:
    src = _csv(tmp_path / "in.csv", [["a"], ["1"]])
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"mode": "column", "column": "b"})
    assert r.errors


def test_excel_keeps_types_and_format(tmp_path: Path) -> None:
    src = tmp_path / "in.xlsx"
    wb = openpyxl.Workbook()
    ws = wb.active
    for row in (["id", "code"], [1, "007"], [2, "008"]):
        ws.append(row)
    wb.create_sheet("Other").append(["x"])
    wb.save(src)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"rows": 1, "fileTitles": ["book"]})
    assert r.names == ["book-part-1.xlsx", "book-part-2.xlsx"]
    rows = [
        [c.value for c in row] for row in openpyxl.load_workbook(r.outputs[1]).active.iter_rows()
    ]
    assert rows == [["id", "code"], [2, "008"]]
    assert r.warnings == ["ONLY_FIRST_SHEET"]
