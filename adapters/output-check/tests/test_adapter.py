"""output-check (ADR 0006): the core's re-check of pdf-redact and file-disarm
output, run in a separate job over copies of the output.

The inputs are real outputs of those two adapters, and the same files with
the thing that should be gone put back."""

from __future__ import annotations

import json
import zipfile
from pathlib import Path
from typing import Any

import pymupdf
import pytest
from converter_conformance.harness import context, load_adapter_class, make_job_dir, run_in_process

ADAPTERS = Path(__file__).resolve().parents[2]
CHECK = ADAPTERS / "output-check"
SECRET = "Zorblax7731"
KEEP = "Visible-Keeper"
BOX = {"page": 1, "x0": 0.05, "y0": 0.1, "x1": 0.95, "y1": 0.16}


def source_pdf(path: Path) -> Path:
    doc = pymupdf.open()
    page = doc.new_page(width=612, height=792)
    page.insert_text((72, 100), f"Account {SECRET} belongs to the client.", fontsize=12)
    page.insert_text((72, 300), f"{KEEP} stays readable.", fontsize=12)
    doc.save(path)
    return path


def redacted(tmp_path: Path) -> Path:
    """A real pdf-redact output: the term and the box are gone."""
    run = run_in_process(
        ADAPTERS / "pdf-redact",
        tmp_path / "redact",
        [source_pdf(tmp_path / "source.pdf")],
        {"terms": [SECRET], "boxes": [BOX]},
    )
    assert run.errors == [], run.errors
    return run.outputs[0]


def redaction_params(**more: Any) -> dict[str, Any]:
    return {
        "verifier": "redaction",
        "expectedPages": 1,
        "fill": [0, 0, 0],
        "terms": [SECRET],
        "regions": [BOX],
        **more,
    }


def check(tmp_path: Path, inputs: list[Path], params: dict[str, Any]):
    """What the core's check job does: execute only. It sends no validate, so a
    file that fails validation (a corrupt output) must still end in a report."""
    job_dir, rel = make_job_dir(tmp_path / "check", inputs)
    ctx, sent = context(job_dir, rel, None)
    outputs = load_adapter_class(CHECK)().execute_conversion(ctx, params)
    assert len(outputs) == 1
    return sent, json.loads(outputs[0].path.read_text("utf-8"))


def docm(path: Path) -> Path:
    """A document with a macro project, named as the core names an output."""
    ct = "http://schemas.openxmlformats.org/package/2006/content-types"
    pkg = "http://schemas.openxmlformats.org/package/2006/relationships"
    rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
    main = "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
    parts = {
        "[Content_Types].xml": (
            f'<?xml version="1.0"?><Types xmlns="{ct}">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package'
            '.relationships+xml"/>'
            '<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/>'
            f'<Override PartName="/word/document.xml" ContentType="{main}"/></Types>'
        ),
        "_rels/.rels": (
            f'<?xml version="1.0"?><Relationships xmlns="{pkg}">'
            f'<Relationship Id="rId1" Type="{rel}/officeDocument" Target="word/document.xml"/>'
            "</Relationships>"
        ),
        "word/document.xml": (
            f'<?xml version="1.0"?><w:document xmlns:w="{w}"><w:body>'
            "<w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"
        ),
        "word/_rels/document.xml.rels": (
            f'<?xml version="1.0"?><Relationships xmlns="{pkg}">'
            f'<Relationship Id="rId1" Type="{rel}/vbaProject" Target="vbaProject.bin"/>'
            "</Relationships>"
        ),
        "word/vbaProject.bin": b"\xd0\xcf\x11\xe0 macro",
    }
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return path


def disarmed(tmp_path: Path) -> Path:
    run = run_in_process(
        ADAPTERS / "file-disarm",
        tmp_path / "disarm",
        [docm(tmp_path / "macro.docx")],
        {"fileTitles": ["macro"]},
    )
    assert run.errors == [], run.errors
    return run.outputs[0]


# ------------------------------------------------------------- redaction


def test_a_real_redaction_output_passes(tmp_path: Path) -> None:
    _, report = check(tmp_path, [redacted(tmp_path)], redaction_params())
    assert report["passed"] is True, report
    assert report["failed"] == []
    ids = [c["id"] for c in report["inputs"][0]["checks"]]
    assert ids == [f"V{n}" for n in range(1, 9)]


def test_a_pdf_that_still_has_the_term_fails(tmp_path: Path) -> None:
    """The case a worker taken over would hide: the file is the unredacted
    original, whatever the tool's worker says about it."""
    _, report = check(tmp_path, [source_pdf(tmp_path / "source.pdf")], redaction_params())
    assert report["passed"] is False
    assert {"V1", "V2", "V3", "V6"} <= set(report["failed"]), report


def test_the_wrong_page_count_fails(tmp_path: Path) -> None:
    _, report = check(tmp_path, [redacted(tmp_path)], redaction_params(expectedPages=2))
    assert report["failed"] == ["V8"]


def test_text_left_under_a_box_fails_without_the_captured_text(tmp_path: Path) -> None:
    """V1 and V2 work from the user's boxes alone, which is all the core has."""
    src = source_pdf(tmp_path / "source.pdf")
    _, report = check(tmp_path, [src], redaction_params(terms=[]))
    assert {"V1", "V2"} <= set(report["failed"]), report


def test_a_file_that_is_not_a_pdf_is_a_failed_check_not_a_crash(tmp_path: Path) -> None:
    bad = tmp_path / "bad.pdf"
    bad.write_bytes(b"%PDF-1.7 not really")
    _, report = check(tmp_path, [bad], redaction_params())
    assert report["passed"] is False
    assert report["failed"] == ["V0"]


def test_redaction_checks_exactly_one_file(tmp_path: Path) -> None:
    out = redacted(tmp_path)
    second = tmp_path / "second.pdf"
    second.write_bytes(out.read_bytes())
    run = run_in_process(CHECK, tmp_path / "check", [out, second], redaction_params())
    assert run.errors and run.outputs == []


def test_validation_refuses_a_corrupt_input_with_a_structured_error(tmp_path: Path) -> None:
    bad = tmp_path / "bad.pdf"
    bad.write_bytes(b"%PDF-1.7 not really")
    run = run_in_process(CHECK, tmp_path / "check", [bad], redaction_params())
    assert [e.code for e in run.errors] == ["E_INPUT_CORRUPT"]


# ---------------------------------------------------------------- disarm


def test_a_disarmed_file_passes(tmp_path: Path) -> None:
    _, report = check(tmp_path, [disarmed(tmp_path)], {"verifier": "activeContent"})
    assert report["passed"] is True, report
    assert report["inputs"][0]["checks"] == [{"id": "activeContent", "passed": True, "count": 0}]


def test_a_docx_with_a_macro_project_fails(tmp_path: Path) -> None:
    _, report = check(tmp_path, [docm(tmp_path / "macro.docx")], {"verifier": "activeContent"})
    assert report["passed"] is False
    assert report["failed"] == ["activeContent"]
    assert report["inputs"][0]["checks"][0]["count"] >= 1


def test_every_file_is_checked_and_one_bad_one_fails_the_run(tmp_path: Path) -> None:
    clean = disarmed(tmp_path)
    bad = docm(tmp_path / "macro.docx")
    _, report = check(tmp_path, [clean, bad], {"verifier": "activeContent"})
    assert [r["checks"][0]["passed"] for r in report["inputs"]] == [True, False]
    assert report["passed"] is False


# --------------------------------------------------------------- privacy


def test_no_document_text_in_the_report_or_the_messages(tmp_path: Path) -> None:
    sent, report = check(tmp_path, [source_pdf(tmp_path / "source.pdf")], redaction_params())
    assert report["passed"] is False  # the secret is in the file, so a leak would be easy
    text = json.dumps(report) + "".join(str(m) for m in sent)
    assert SECRET not in text
    assert KEEP not in text
    assert "Account" not in text


def test_the_report_is_the_same_on_a_repeat_run(tmp_path: Path) -> None:
    out = redacted(tmp_path)
    one = check(tmp_path / "a", [out], redaction_params())[1]
    two = check(tmp_path / "b", [out], redaction_params())[1]
    assert one == two


def test_the_verifier_is_one_of_two_fixed_files_never_a_path() -> None:
    schema = json.loads((CHECK / "params.schema.json").read_text("utf-8"))
    assert schema["properties"]["verifier"]["enum"] == ["redaction", "activeContent"]
    assert schema["additionalProperties"] is False
    assert "captured" not in schema["properties"]


@pytest.mark.parametrize("tool", ["pdf-redact", "file-disarm"])
def test_the_tools_declare_their_recheck(tool: str) -> None:
    import yaml  # noqa: PLC0415

    manifest = yaml.safe_load((ADAPTERS / tool / "manifest.yaml").read_text("utf-8"))
    assert manifest["recheck"] == {"pdf-redact": "redaction", "file-disarm": "activeContent"}[tool]
    assert (ADAPTERS / tool / "verify.py").is_file()
