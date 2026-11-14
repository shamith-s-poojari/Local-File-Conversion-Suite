"""Acceptance tests for data-combine (ADR 0004)."""

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


def _xlsx(path: Path, sheets: dict[str, list[list[object]]]) -> Path:
    wb = openpyxl.Workbook()
    wb.remove(wb.active)
    for name, rows in sheets.items():
        ws = wb.create_sheet(name)
        for r in rows:
            ws.append(r)
    wb.save(path)
    return path


def test_rows_are_matched_by_column_name(tmp_path: Path) -> None:
    a = _csv(tmp_path / "a.csv", [["name", "score"], ["Ada", "9"], ["Bo", "007"]])
    b = _xlsx(tmp_path / "b.xlsx", {"Q1": [["score", "name", "city"], [7, "Cy", "Oslo"]]})
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [a, b],
        {"to": "csv", "addSource": True, "fileTitles": ["a", "b"]},
    )
    assert not r.errors
    assert r.names == ["combined.csv"]
    rows = list(csv.reader(r.outputs[0].read_text("utf-8").splitlines()))
    assert rows == [
        ["Source file", "name", "score", "city"],
        ["a", "Ada", "9", ""],
        ["a", "Bo", "007", ""],
        ["b", "Cy", "7", "Oslo"],
    ]
    assert r.warnings == ["COLUMNS_MISMATCHED"]


def test_excel_output_types_csv_numbers_only(tmp_path: Path) -> None:
    a = _csv(tmp_path / "a.csv", [["id", "code"], ["1", "007"]])
    b = _xlsx(tmp_path / "b.xlsx", {"S": [["id", "code"], [2, "008"]]})
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"to": "xlsx"})
    ws = openpyxl.load_workbook(r.outputs[0]).active
    assert [[c.value for c in row] for row in ws.iter_rows()] == [
        ["id", "code"],
        [1, "007"],
        [2, "008"],
    ]
    assert r.warnings == []


def test_each_file_as_its_own_sheet(tmp_path: Path) -> None:
    a = _csv(tmp_path / "a.csv", [["x"], ["1"]])
    b = _xlsx(tmp_path / "b.xlsx", {"One": [["y"], [2]], "Two": [["z"], [3]]})
    r = run_in_process(
        ADAPTER,
        tmp_path / "job",
        [a, b],
        {"to": "xlsx", "mode": "sheets", "fileTitles": ["a", "b"]},
    )
    wb = openpyxl.load_workbook(r.outputs[0])
    assert wb.sheetnames == ["a", "b - One", "b - Two"]


def test_without_column_names_rows_are_appended(tmp_path: Path) -> None:
    a = _csv(tmp_path / "a.csv", [["1", "2"]])
    b = _csv(tmp_path / "b.csv", [["3", "4", "5"]])
    r = run_in_process(ADAPTER, tmp_path / "job", [a, b], {"to": "csv", "firstRowIsHeader": False})
    assert r.outputs[0].read_text("utf-8") == "1,2\n3,4,5\n"
