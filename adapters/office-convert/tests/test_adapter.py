"""Acceptance tests for office-convert (ADR 0003). Need LibreOffice."""

from __future__ import annotations

import os
import re
import zipfile
from pathlib import Path

import pytest
import yaml
from converter_conformance import files, validate
from converter_conformance.harness import load_adapter_class, run_in_process
from converter_sdk import AdapterError

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)
pytestmark = pytest.mark.skipif(files.soffice() is None, reason="LibreOffice isn't installed")

SOURCES = sorted({s for s, _ in CLS.EDGES})
CALC = {"xlsx", "xls", "ods", "csv"}
IMPRESS = {"pptx", "ppt", "odp"}
IMAGES = {"png", "jpg", "webp"}
TWO_PAGES = IMPRESS | {"xlsx", "xls", "ods", "pdf"}
"""Fixtures that print as two pages (two slides, two sheets)."""


def _cover(pairs: list[tuple[str, str]]) -> list[tuple[str, str]]:
    """Pairs in which every source and every target appears at least once."""
    chosen: list[tuple[str, str]] = []
    seen_src: set[str] = set()
    seen_to: set[str] = set()
    for s, t in pairs:
        if s not in seen_src or t not in seen_to:
            chosen.append((s, t))
            seen_src.add(s)
            seen_to.add(t)
    return chosen


ALL_PAIRS = sorted(CLS.EDGES)
PAIRS = ALL_PAIRS if os.environ.get("LFCS_FULL_MATRIX") else _cover(ALL_PAIRS)
"""Every pair takes about eight minutes (LibreOffice starts for each), so by
default a cover of them runs; ``LFCS_FULL_MATRIX=1`` runs them all."""


@pytest.fixture(scope="module")
def sources(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    d = tmp_path_factory.mktemp("office")
    return {f: files.build("doc-pdf-report" if f == "pdf" else f"doc-{f}", d) for f in SOURCES}


def convert(tmp_path: Path, src: Path, to: str, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["report"], **params}
    )


def words(path: Path, fmt: str) -> str:
    """The text of an output, roughly, to check the content came through."""
    data = path.read_bytes()
    if fmt == "pdf":
        import pypdfium2 as pdfium  # noqa: PLC0415

        doc = pdfium.PdfDocument(path)
        try:
            return " ".join(doc[n].get_textpage().get_text_range() for n in range(len(doc)))
        finally:
            doc.close()
    if fmt in ("docx", "xlsx", "pptx", "odt", "ods", "odp", "epub"):
        with zipfile.ZipFile(path) as z:
            xml = " ".join(
                z.read(n).decode("utf-8", "replace")
                for n in z.namelist()
                if n.endswith((".xml", "html"))
            )
        return re.sub(r"<[^>]+>", " ", xml)
    if fmt in ("doc", "xls", "ppt"):
        # Compound files keep text as UTF-16 or Latin-1.
        return data.decode("utf-16-le", "replace") + data.decode("latin-1")
    return data.decode("utf-8-sig", "replace")


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize(("src", "to"), PAIRS)
def test_every_pair(tmp_path: Path, sources, src: str, to: str) -> None:
    r = convert(tmp_path, sources[src], to)
    assert not r.errors, r.errors
    assert r.outputs
    for out in r.outputs:
        validate.check(out, to)
    if to == "csv":
        assert r.names == ["report-Sales.csv", "report-Notes.csv"] or r.names == ["report.csv"]
    elif to in IMAGES and src in TWO_PAGES:
        assert r.names == [f"report-page-1.{to}", f"report-page-2.{to}"]
    else:
        assert r.names == [f"report.{to}"]
    if to in IMAGES:
        return
    keyword = "North" if src in CALC else "Lisbon"
    assert keyword in words(r.outputs[0], to) or any(keyword in words(o, to) for o in r.outputs)


def test_every_sheet_becomes_its_own_csv(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["xlsx"], "csv")
    assert r.names == ["report-Sales.csv", "report-Notes.csv"]
    assert {"FORMULAS_DROPPED", "SHEETS_SPLIT"} <= set(r.warnings)
    sales = r.outputs[0].read_text("utf-8").splitlines()
    # Formulas are saved as their values.
    assert sales[:2] == ["Region,Q1,Q2,Total", "North,1200,1350,2550"]


def test_spreadsheet_to_text_has_every_sheet(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["xlsx"], "txt")
    text = r.outputs[0].read_text("utf-8")
    assert text.startswith("Sales\n\nRegion\tQ1\tQ2\tTotal\nNorth\t1200\t1350\t2550\n")
    assert "\n\n\nNotes\n\nNote\nFigures are in euros\n" in text


def test_pdf_pages_become_slides(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["pdf"], "pptx")
    with zipfile.ZipFile(r.outputs[0]) as z:
        slides = [n for n in z.namelist() if re.fullmatch(r"ppt/slides/slide\d+\.xml", n)]
    assert len(slides) == 2
    assert "LAYOUT_APPROXIMATED" in r.warnings


def test_encrypted_pdf_needs_its_password(tmp_path: Path) -> None:
    from converter_conformance import corpus  # noqa: PLC0415

    src = corpus.build("encrypted", tmp_path / "in")
    r = convert(tmp_path / "a", src, "pptx")
    assert [e.code for e in r.errors] == ["E_PASSWORD_REQUIRED"]
    r = run_in_process(
        ADAPTER, tmp_path / "b", [src], {"to": "pptx", "fileTitles": ["report"]},
        {"password": corpus.SENTINEL_PASSWORD},
    )  # fmt: skip
    assert not r.errors and r.outputs


def test_csv_formulas_stay_text(tmp_path: Path) -> None:
    import openpyxl  # noqa: PLC0415

    src = tmp_path / "evil.csv"
    src.write_text('Name,Link\nAda,"=HYPERLINK(""http://x"",""y"")"\nBob,=1+1\n', "utf-8")
    ws = openpyxl.load_workbook(convert(tmp_path, src, "xlsx").outputs[0]).active
    assert ws.cell(2, 2).data_type == "s" and ws.cell(2, 2).value.startswith("=HYPERLINK")
    assert ws.cell(3, 2).data_type == "s" and ws.cell(3, 2).value == "=1+1"


def test_csv_quotes_and_commas_survive(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["csv"], "xlsx")
    import openpyxl  # noqa: PLC0415

    ws = openpyxl.load_workbook(r.outputs[0]).active
    rows = [[c.value for c in row] for row in ws.iter_rows()]
    assert rows[0] == ["Region", "Q1", "Q2"]
    assert rows[3][0] == "East, coast"


def test_markdown_headings_become_word_headings(tmp_path: Path, sources) -> None:
    import docx  # noqa: PLC0415

    r = convert(tmp_path, sources["md"], "docx")
    d = docx.Document(str(r.outputs[0]))
    styles = {p.text: p.style.name for p in d.paragraphs if p.text}
    assert styles.get("Quarterly report", "").startswith("Heading")
    assert d.tables and d.tables[0].cell(1, 0).text == "North"


def test_word_to_markdown_keeps_structure(tmp_path: Path, sources) -> None:
    r = convert(tmp_path, sources["docx"], "md")
    md = r.outputs[0].read_text("utf-8")
    assert md.startswith("# Quarterly report")
    assert "| Region | Sales |" in md
    assert "LAYOUT_APPROXIMATED" in r.warnings


def test_image_options_are_used(tmp_path: Path, sources) -> None:
    from PIL import Image  # noqa: PLC0415

    small = convert(tmp_path / "a", sources["docx"], "png", dpi=72).outputs[0]
    big = convert(tmp_path / "b", sources["docx"], "png", dpi=150).outputs[0]
    with Image.open(small) as a, Image.open(big) as b:
        assert b.width == pytest.approx(a.width * 150 / 72, abs=2)


def test_pdf_output_repeats_exactly(tmp_path: Path, sources) -> None:
    a = convert(tmp_path / "a", sources["docx"], "pdf").outputs[0].read_bytes()
    b = convert(tmp_path / "b", sources["docx"], "pdf").outputs[0].read_bytes()
    assert a == b


@pytest.mark.parametrize(
    "name", ["corrupt-docx", "truncated-docx", "corrupt-xlsx", "truncated-xlsx"]
)
def test_damaged_files_are_reported_as_damaged(tmp_path: Path, name: str) -> None:
    src = files.build(name, tmp_path / "in")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "pdf")
    assert e.value.code == "E_INPUT_CORRUPT"


def test_empty_file_is_refused_before_starting(tmp_path: Path) -> None:
    src = tmp_path / "empty.docx"
    src.write_bytes(b"")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "pdf")
    assert e.value.code == "E_INPUT_CORRUPT"
