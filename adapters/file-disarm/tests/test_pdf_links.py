"""PRIV-5: a PDF link that isn't plain web (file:, smb:, javascript:) is not
"just a link", and the verifier looks for it without the SDK's detector."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pikepdf
import pytest
from converter_conformance.harness import run_in_process
from converter_sdk.active import active_findings
from pikepdf import Array, Dictionary, Name, String

ADAPTER = Path(__file__).resolve().parents[1]
WEB = ["https://example.org/a", "mailto:a@example.org", "tel:+15551234"]
NOT_WEB = [
    "file:///etc/passwd",
    "smb://server/share/x",
    "javascript:alert(1)",
    "  JaVa\tScript:alert(1)",
    "\\\\server\\share",
    "relative/page.html",
]


def link_pdf(path: Path, uris: list[str]) -> Path:
    pdf = pikepdf.new()
    pdf.add_blank_page(page_size=(200, 200))
    annots = [
        pdf.make_indirect(
            Dictionary(
                Type=Name.Annot,
                Subtype=Name.Link,
                Rect=Array([0, 10 * i, 50, 10 * i + 9]),
                A=Dictionary(S=Name.URI, URI=String(uri)),
            )
        )
        for i, uri in enumerate(uris)
    ]
    pdf.pages[0].obj.Annots = Array(annots)
    pdf.save(path)
    return path


def uris_in(path: Path) -> list[str]:
    with pikepdf.open(path) as pdf:
        return [str(a.A.URI) for a in pdf.pages[0].get("/Annots", []) if "/A" in a]


def verify(job: Path, relative: str, **extra) -> dict:
    done = subprocess.run(  # noqa: S603 - our interpreter and our verifier
        [sys.executable, "-I", str(ADAPTER / "verify.py")],
        cwd=job,
        input=json.dumps({"output": relative, **extra}).encode(),
        capture_output=True,
        check=False,
    )
    return json.loads(done.stdout)


def test_the_detector_sees_links_that_are_not_web(tmp_path: Path) -> None:
    assert active_findings(link_pdf(tmp_path / "w.pdf", WEB), "pdf") == []
    for uri in NOT_WEB:
        assert active_findings(link_pdf(tmp_path / "n.pdf", [uri]), "pdf"), uri


def test_disarm_keeps_web_links_and_removes_the_others(tmp_path: Path) -> None:
    src = link_pdf(tmp_path / "a.pdf", [*WEB, *NOT_WEB])
    run = run_in_process(ADAPTER, tmp_path / "job", [src], {"fileTitles": ["a"]})
    assert run.errors == []
    assert uris_in(run.outputs[0]) == WEB
    assert "EXTERNAL_LINKS_REMOVED" in run.warnings
    assert active_findings(run.outputs[0], "pdf") == []


def test_the_verifier_fails_a_leftover_non_web_uri(tmp_path: Path) -> None:
    job = tmp_path / "job"
    (job / "out").mkdir(parents=True)
    link_pdf(job / "out" / "ok.pdf", WEB)
    assert verify(job, "out/ok.pdf") == {"findings": []}
    link_pdf(job / "out" / "bad.pdf", ["file:///c:/windows/system32/calc.exe"])
    report = verify(job, "out/bad.pdf")
    assert "PDF URI left" in [f["detail"] for f in report["findings"]]
    # With "remove links" every /URI counts, web or not.
    assert verify(job, "out/ok.pdf", removeLinks=True)["findings"]


@pytest.mark.parametrize(
    "name", ["Launch", "JavaScript", "GoToR", "GoToE", "SubmitForm", "ImportData"]
)
def test_the_verifier_finds_action_types_anywhere(tmp_path: Path, name: str) -> None:
    """Deep inside a direct array in an object the SDK sweep doesn't treat as a holder."""
    job = tmp_path / "job"
    (job / "out").mkdir(parents=True)
    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.Root.Odd = Array([Dictionary(Inner=Array([Dictionary(S=Name("/" + name))]))])
    pdf.save(job / "out" / "x.pdf")
    kinds = [f["detail"] for f in verify(job, "out/x.pdf")["findings"]]
    assert f"PDF {name} left" in kinds


def test_the_verifier_finds_js_and_richmedia_keys(tmp_path: Path) -> None:
    job = tmp_path / "job"
    (job / "out").mkdir(parents=True)
    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.Root.Odd = Dictionary(JS=String("x()"), Kind=Name.RichMedia)
    pdf.save(job / "out" / "x.pdf")
    kinds = {f["detail"] for f in verify(job, "out/x.pdf")["findings"]}
    assert {"PDF JS left", "PDF RichMedia left"} <= kinds
