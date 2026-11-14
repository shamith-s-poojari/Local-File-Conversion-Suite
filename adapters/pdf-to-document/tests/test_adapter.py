"""Acceptance tests for pdf-to-document (ADR 0003)."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml
from converter_conformance import corpus, files, validate
from converter_conformance.harness import load_adapter_class, run_in_process
from converter_sdk import AdapterError

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)


@pytest.fixture(scope="module")
def report(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return files.build("doc-pdf-report", tmp_path_factory.mktemp("pdf"))


def convert(tmp_path: Path, srcs: list[Path], to: str, secrets=None, **params):
    titles = [s.stem for s in srcs]
    return run_in_process(
        ADAPTER, tmp_path / "job", srcs, {"to": to, "fileTitles": titles, **params}, secrets
    )


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize("to", ["docx", "xlsx", "md", "html", "txt"])
def test_every_target_is_valid_and_keeps_the_words(tmp_path: Path, report: Path, to: str) -> None:
    r = convert(tmp_path, [report], to)
    assert not r.errors, r.errors
    assert r.names == [f"{report.stem}.{to}"]
    validate.check(r.outputs[0], to)
    if to == "docx":
        import docx  # noqa: PLC0415

        d = docx.Document(str(r.outputs[0]))
        text = " ".join(p.text for p in d.paragraphs)
        assert "Lisbon" in text and "Quarterly report" in text
        assert [[c.text for c in row.cells] for row in d.tables[0].rows][1] == [
            "North",
            "1200",
            "1350",
        ]
    elif to == "xlsx":
        import openpyxl  # noqa: PLC0415

        ws = openpyxl.load_workbook(r.outputs[0]).active
        assert ws.title == "Page 1"
        assert [[c.value for c in row] for row in ws.iter_rows()] == [
            ["Region", "Q1", "Q2"], ["North", 1200, 1350], ["South", 980, 1010],
        ]  # fmt: skip
    else:
        assert "Lisbon" in r.outputs[0].read_text("utf-8")


def test_markdown_has_headings_emphasis_and_tables(tmp_path: Path, report: Path) -> None:
    r = convert(tmp_path, [report], "md")
    md = r.outputs[0].read_text("utf-8")
    assert md.startswith("# Quarterly report\n\n")
    assert "\n## Outlook\n" in md
    assert "Totals are **final** for every region." in md
    assert "| Region | Q1 | Q2 |\n|---|---|---|\n| North | 1200 | 1350 |" in md
    # The table's words appear once: as a table, not also as paragraphs.
    assert md.count("North") == 1
    assert "IMAGES_DROPPED" in r.warnings


def test_html_is_a_page_with_structure(tmp_path: Path, report: Path) -> None:
    html = convert(tmp_path, [report], "html").outputs[0].read_text("utf-8")
    assert html.startswith("<!DOCTYPE html>")
    assert "<h1>Quarterly report</h1>" in html and "<h2>Outlook</h2>" in html
    assert "<strong>final</strong>" in html
    assert "<th>Region</th>" in html and "<td>North</td>" in html
    assert "<script" not in html and "<img" not in html


def test_page_ranges_choose_pages(tmp_path: Path, report: Path) -> None:
    md = convert(tmp_path, [report], "md", pages="2").outputs[0].read_text("utf-8")
    assert "Lisbon" in md and "Quarterly" not in md


def test_one_output_per_input(tmp_path: Path) -> None:
    a = corpus.build("text-3", tmp_path / "in")
    b = corpus.build("text-1", tmp_path / "in")
    r = convert(tmp_path, [a, b], "txt")
    assert r.names == ["text-3.txt", "text-1.txt"]
    assert r.sources == [0, 1]
    assert r.outputs[0].read_text("utf-8").count("\n\n") >= 2


def test_formula_like_text_stays_text(tmp_path: Path) -> None:
    import openpyxl  # noqa: PLC0415
    import pymupdf  # noqa: PLC0415

    src = tmp_path / "sheet.pdf"
    doc = pymupdf.open()
    page = doc.new_page()
    for r, row in enumerate([["Name", "Link"], ["Ada", '=HYPERLINK("http://x","y")']]):
        for c, text in enumerate(row):
            rect = pymupdf.Rect(72 + c * 200, 72 + r * 24, 272 + c * 200, 96 + r * 24)
            page.draw_rect(rect, color=(0, 0, 0))
            page.insert_text((rect.x0 + 4, rect.y1 - 7), text, fontsize=10)
    doc.save(src)
    doc.close()
    ws = openpyxl.load_workbook(convert(tmp_path, [src], "xlsx").outputs[0]).active
    cell = ws.cell(2, 2)
    assert cell.value.startswith("=HYPERLINK") and cell.data_type == "s"


def test_scans_have_no_text_to_convert(tmp_path: Path) -> None:
    scan = files.build("doc-pdf-scan", tmp_path / "in")
    for to in ["txt", "md", "html", "xlsx"]:
        with pytest.raises(AdapterError) as e:
            convert(tmp_path / to, [scan], to)
        assert e.value.code == "E_NO_TEXT"
    r = convert(tmp_path / "docx", [scan], "docx")
    assert r.outputs and "NO_TEXT_LAYER" in r.warnings


def test_encrypted_pdf_needs_its_password(tmp_path: Path) -> None:
    src = corpus.build("encrypted", tmp_path / "in")
    r = convert(tmp_path / "a", [src], "txt")
    assert [e.code for e in r.errors] == ["E_PASSWORD_REQUIRED"]
    secrets = {"password": corpus.SENTINEL_PASSWORD}
    r = convert(tmp_path / "b", [src], "txt", secrets=secrets)
    assert not r.errors and r.outputs[0].read_text("utf-8").strip()
    r = convert(tmp_path / "c", [src], "docx", secrets=secrets)
    assert not r.errors and r.outputs


def test_wrong_target_is_refused(tmp_path: Path, report: Path) -> None:
    r = convert(tmp_path, [report], "pptx")
    assert [(e.param, e.detail.root) for e in r.errors] == [
        ("to", {"reason": "unsupportedConversion"})
    ]
