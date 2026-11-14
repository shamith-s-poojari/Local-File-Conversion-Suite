"""Acceptance tests for data-convert (ADR 0003)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import yaml
from converter_conformance import files, validate
from converter_conformance.harness import load_adapter_class, run_in_process
from converter_sdk import AdapterError

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)


@pytest.fixture(scope="module")
def sources(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    d = tmp_path_factory.mktemp("data")
    return {f: files.build(f"doc-{f}", d) for f in ["csv", "json", "yaml", "xlsx"]}


def convert(tmp_path: Path, src: Path, to: str, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["data"], **params}
    )


def sheet_rows(path: Path, index: int = 0) -> list[list]:
    import openpyxl  # noqa: PLC0415

    ws = openpyxl.load_workbook(path).worksheets[index]
    return [[c.value for c in row] for row in ws.iter_rows()]


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize(("src", "to"), sorted(CLS.EDGES))
def test_every_pair(tmp_path: Path, sources, src: str, to: str) -> None:
    r = convert(tmp_path, sources[src], to)
    assert not r.errors, r.errors
    for out in r.outputs:
        validate.check(out, to)
    if src == "xlsx" and to == "csv":
        assert r.names == ["data-Sales.csv", "data-Notes.csv"]
    else:
        assert r.names == [f"data.{to}"]
    if to in ("json", "yaml", "csv", "md", "html"):
        assert "North" in r.outputs[0].read_text("utf-8")


def test_csv_to_json_makes_typed_records(tmp_path: Path, sources) -> None:
    out = convert(tmp_path, sources["csv"], "json").outputs[0]
    assert json.loads(out.read_text("utf-8")) == [
        {"Region": "North", "Q1": 1200, "Q2": 1350},
        {"Region": "South", "Q1": 980, "Q2": 1010},
        {"Region": "East, coast", "Q1": 700, "Q2": "7,5"},
    ]


def test_numbers_that_would_lose_something_stay_text(tmp_path: Path) -> None:
    src = tmp_path / "ids.csv"
    src.write_text("id,zip,big,exp,price,neg\n1,007,12345678901234567,1e5,1.50,-3\n", "utf-8")
    [row] = json.loads(convert(tmp_path, src, "json").outputs[0].read_text("utf-8"))
    assert row == {
        "id": 1,
        "zip": "007",
        "big": "12345678901234567",
        "exp": "1e5",
        "price": 1.5,
        "neg": -3,
    }


def test_rows_without_a_header(tmp_path: Path, sources) -> None:
    out = convert(tmp_path, sources["csv"], "json", firstRowIsHeader=False).outputs[0]
    rows = json.loads(out.read_text("utf-8"))
    assert rows[0] == ["Region", "Q1", "Q2"] and rows[1] == ["North", 1200, 1350]


def test_semicolons_and_tabs_are_detected(tmp_path: Path) -> None:
    for name, text in [("semi.csv", "a;b\n1;2,5\n"), ("tabs.csv", "a\tb\n1\tx y\n")]:
        src = tmp_path / name
        src.write_text(text, "utf-8")
        rows = json.loads(
            convert(tmp_path / f"job-{name}", src, "json").outputs[0].read_text("utf-8")
        )
        assert len(rows[0]) == 2, rows


def test_formula_text_never_becomes_a_formula(tmp_path: Path) -> None:
    import openpyxl  # noqa: PLC0415

    src = tmp_path / "evil.csv"
    src.write_text(
        'Name,Link\nAda,"=HYPERLINK(""http://x"",""y"")"\nBob,@SUM(1)\nCy,+1+1\n', "utf-8"
    )
    out = convert(tmp_path, src, "xlsx").outputs[0]
    ws = openpyxl.load_workbook(out).active
    for row in range(2, 5):
        cell = ws.cell(row, 2)
        assert cell.data_type == "s", (cell.value, cell.data_type)
    assert ws.cell(2, 2).value.startswith("=HYPERLINK")


def test_json_records_become_a_sheet_and_nested_values_json_text(tmp_path: Path) -> None:
    src = tmp_path / "people.json"
    src.write_text(
        json.dumps([{"name": "Ada", "tags": ["x", "y"]}, {"name": "Bob", "age": 36}]), "utf-8"
    )
    r = convert(tmp_path, src, "xlsx")
    assert sheet_rows(r.outputs[0]) == [
        ["name", "tags", "age"],
        ["Ada", '["x","y"]', None],
        ["Bob", None, 36],
    ]
    assert "NESTED_DATA_FLATTENED" in r.warnings


def test_an_object_of_lists_becomes_sheets_and_csv_files(tmp_path: Path) -> None:
    src = tmp_path / "book.json"
    src.write_text(json.dumps({"Sales": [{"q": 1}], "Notes": [{"n": "hi"}]}), "utf-8")
    r = convert(tmp_path / "x", src, "xlsx")
    import openpyxl  # noqa: PLC0415

    assert openpyxl.load_workbook(r.outputs[0]).sheetnames == ["Sales", "Notes"]
    r = convert(tmp_path / "c", src, "csv")
    assert r.names == ["data-Sales.csv", "data-Notes.csv"] and "SHEETS_SPLIT" in r.warnings


def test_json_and_yaml_round_trip(tmp_path: Path) -> None:
    data = {
        "name": "Ada",
        "on": "yes",
        "zip": "007",
        "n": [1, 2.5, None, True],
        "deep": {"a": {"b": []}},
    }
    src = tmp_path / "in.json"
    src.write_text(json.dumps(data), "utf-8")
    y = convert(tmp_path / "y", src, "yaml").outputs[0]
    assert yaml.safe_load(y.read_text("utf-8")) == data
    back = convert(tmp_path / "j", y, "json").outputs[0]
    assert json.loads(back.read_text("utf-8")) == data


def test_yaml_specials_become_plain_json(tmp_path: Path) -> None:
    src = tmp_path / "in.yaml"
    src.write_text(
        "# settings\nwhen: 2024-05-01\nraw: !!binary aGk=\n1: one\nbase: &b {x: 1}\nuse: *b\n",
        "utf-8",
    )
    r = convert(tmp_path, src, "json")
    assert json.loads(r.outputs[0].read_text("utf-8")) == {
        "when": "2024-05-01", "raw": "aGk=", "1": "one", "base": {"x": 1}, "use": {"x": 1},
    }  # fmt: skip
    assert "COMMENTS_DROPPED" in r.warnings


def test_yaml_alias_bombs_are_refused(tmp_path: Path) -> None:
    lines = ['a: &a ["x","x","x","x","x","x","x","x","x","x"]']
    for i, name in enumerate("bcdefghi"):
        prev = "abcdefghi"[i]
        lines.append(f"{name}: &{name} [{','.join(['*' + prev] * 10)}]")
    src = tmp_path / "bomb.yaml"
    src.write_text("\n".join(lines) + "\n", "utf-8")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "json")
    assert e.value.code == "E_LIMIT_EXCEEDED"


def test_recursive_yaml_is_refused(tmp_path: Path) -> None:
    src = tmp_path / "loop.yaml"
    src.write_text("a: &a [1, *a]\n", "utf-8")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "json")
    assert e.value.code == "E_INPUT_CORRUPT"


def test_unsafe_yaml_tags_are_refused(tmp_path: Path) -> None:
    src = tmp_path / "evil.yaml"
    src.write_text("x: !!python/object/apply:os.system ['echo hi']\n", "utf-8")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "json")
    assert e.value.code == "E_INPUT_CORRUPT"


def test_excel_sheets_to_json_by_name_with_formulas_noted(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["xlsx"], "json")
    data = json.loads(r.outputs[0].read_text("utf-8"))
    assert list(data) == ["Sales", "Notes"]
    assert data["Sales"][0]["Region"] == "North" and data["Sales"][0]["Q1"] == 1200
    assert data["Notes"] == [{"Note": "Figures are in euros"}]
    assert "FORMULAS_DROPPED" in r.warnings


def test_csv_to_markdown_and_html_keep_the_text_as_written(tmp_path: Path) -> None:
    src = tmp_path / "p.csv"
    src.write_text("item,price\npipe | tee,1.50\n<b>x</b>,2\n", "utf-8")
    md = convert(tmp_path / "m", src, "md").outputs[0].read_text("utf-8")
    assert "| pipe \\| tee | 1.50 |" in md
    html = convert(tmp_path / "h", src, "html").outputs[0].read_text("utf-8")
    assert "<td>&lt;b&gt;x&lt;/b&gt;</td>" in html and "<th>item</th>" in html


def test_csv_separator_can_be_chosen(tmp_path: Path, sources) -> None:
    out = convert(tmp_path, sources["json"], "csv", csvDelimiter="semicolon").outputs[0]
    assert out.read_text("utf-8").splitlines()[:2] == ["Region;Q1;Q2", "North;1200;1350"]


def test_legacy_encodings_are_read(tmp_path: Path) -> None:
    src = tmp_path / "old.csv"
    src.write_bytes("name,city\nRené,Montréal\n".encode("cp1252"))
    rows = json.loads(convert(tmp_path, src, "json").outputs[0].read_text("utf-8"))
    assert rows == [{"name": "René", "city": "Montréal"}]


@pytest.mark.parametrize("name", ["corrupt-xlsx", "truncated-xlsx"])
def test_damaged_workbooks(tmp_path: Path, name: str) -> None:
    src = files.build(name, tmp_path / "in")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "json")
    assert e.value.code == "E_INPUT_CORRUPT"


def test_broken_json_is_damaged(tmp_path: Path) -> None:
    src = tmp_path / "bad.json"
    src.write_text('{"a": [1, 2', "utf-8")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "yaml")
    assert e.value.code == "E_INPUT_CORRUPT"
