"""Detection and removal of active content (converter_sdk.active, converter_sdk.pdf)."""

# ruff: noqa: E501 - long XML literals

from __future__ import annotations

import zipfile
from pathlib import Path

import pikepdf
import pytest
from converter_sdk import AdapterError
from converter_sdk.active import (
    active_findings,
    disarm_markup,
    disarm_ooxml,
    parse_xml,
    pdf_active_findings,
)
from converter_sdk.markup import check_zip
from converter_sdk.pdf import remove_active_content, scan_active_content
from lxml import etree
from pikepdf import Array, Dictionary, Name, String

PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
CT = "http://schemas.openxmlformats.org/package/2006/content-types"
MAIN_DOCM = "application/vnd.ms-word.document.macroEnabled.main+xml"
MAIN_DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"


def kinds(findings) -> dict[str, str | None]:
    return {f.detail: f.kind for f in findings}


# ------------------------------------------------------------------ PDF


def js(code: str = "app.alert(1)") -> Dictionary:
    return Dictionary(S=Name.JavaScript, JS=String(code))


def make_pdf(path: Path) -> Path:
    pdf = pikepdf.new()
    pdf.add_blank_page(page_size=(200, 200))
    pdf.add_blank_page(page_size=(200, 200))
    page = pdf.pages[0]
    spec = pdf.make_indirect(
        pikepdf.Dictionary(Type=Name.Filespec, F=String("a.txt"), UF=String("a.txt"))
    )
    stream = pikepdf.Stream(pdf, b"hello")
    spec.EF = Dictionary(F=stream)
    pdf.Root.Names = Dictionary(
        JavaScript=pdf.make_indirect(
            Dictionary(Names=Array([String("x"), pdf.make_indirect(js())]))
        ),
        EmbeddedFiles=pdf.make_indirect(Dictionary(Names=Array([String("a.txt"), spec]))),
        Dests=Dictionary(Names=Array([])),
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
    uri = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Link,
            Rect=Array([60, 0, 90, 50]),
            A=Dictionary(S=Name.URI, URI=String("https://example.com")),
        )
    )
    attach = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.FileAttachment,
            Rect=Array([0, 60, 20, 80]),
            FS=spec,
        )
    )
    page.Annots = Array([launch, uri, attach])
    page.AA = Dictionary(O=js())
    pdf.Root.Outlines = pdf.make_indirect(Dictionary(Type=Name.Outlines, Count=0))
    pdf.save(path)
    return path


def test_pdf_findings_are_by_kind(tmp_path: Path) -> None:
    path = make_pdf(tmp_path / "a.pdf")
    found = kinds(active_findings(path, "pdf"))
    assert found["JavaScript"] == "activeContent"
    assert found["Runs actions when opened"] == "activeContent"
    assert found["Opens another program"] == "activeContent"
    assert found["2 embedded files"] == "embeddedFiles"  # the names tree entry and the annotation


def test_pdf_removal_leaves_no_findings_and_keeps_links(tmp_path: Path) -> None:
    path = make_pdf(tmp_path / "a.pdf")
    with pikepdf.open(path) as pdf:
        counts = remove_active_content(pdf)
        assert counts["javascript"] >= 2 and counts["launch"] == 1 and counts["embedded"] >= 2
        assert pdf_active_findings(pdf) == []
        pdf.save(tmp_path / "b.pdf")
    with pikepdf.open(tmp_path / "b.pdf") as pdf:
        assert scan_active_content(pdf) == {}
        annots = pdf.pages[0].Annots
        assert len(annots) == 2  # the web link stays, the attachment went
        assert any(a.get(Name.A, {}).get(Name.S) == Name.URI for a in annots)
        assert Name.OpenAction not in pdf.Root
        assert len(pdf.pages) == 2


def test_pdf_remove_links_drops_uri_actions(tmp_path: Path) -> None:
    path = make_pdf(tmp_path / "a.pdf")
    with pikepdf.open(path) as pdf:
        counts = remove_active_content(pdf, remove_links=True)
        assert counts["links"] == 1
        assert not any(Name.A in a for a in pdf.pages[0].Annots)


def test_pdf_goto_open_action_and_chained_actions(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.Root.OpenAction = Array([pdf.pages[0].obj, Name.Fit])
    assert scan_active_content(pdf) == {}
    pdf.Root.OpenAction = Dictionary(S=Name.GoTo, D=Array([pdf.pages[0].obj, Name.Fit]), Next=js())
    assert scan_active_content(pdf) == {"javascript": 1}
    counts = remove_active_content(pdf)
    assert counts == {"javascript": 1}
    assert pdf.Root.OpenAction.S == Name.GoTo and Name.Next not in pdf.Root.OpenAction


def test_pdf_form_actions_xfa_and_outline(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    widget = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Widget,
            FT=Name.Btn,
            T=String("b"),
            Rect=Array([0, 0, 10, 10]),
            A=Dictionary(S=Name.SubmitForm, F=Dictionary(FS=Name.URL, F=String("http://x"))),
            AA=Dictionary(E=Dictionary(S=Name.Hide, T=String("b"))),
        )
    )
    pdf.pages[0].Annots = Array([widget])
    pdf.Root.AcroForm = Dictionary(Fields=Array([widget]), XFA=Array([String("xdp")]))
    item = pdf.make_indirect(
        Dictionary(Title=String("t"), A=Dictionary(S=Name.GoToR, F=String("other.pdf")))
    )
    pdf.Root.Outlines = pdf.make_indirect(
        Dictionary(Type=Name.Outlines, First=item, Last=item, Count=1)
    )
    found = kinds(pdf_active_findings(pdf))
    assert "Sends or loads form data" in found and "XFA form" in found
    assert found["Opens other files"] == "externalLinks"
    counts = remove_active_content(pdf)
    assert counts["formActions"] >= 3 and counts["xfa"] == 1 and counts["remote"] == 1
    assert pdf_active_findings(pdf) == []


def test_pdf_embedded_media_annotations(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    annots = [
        pdf.make_indirect(
            Dictionary(Type=Name.Annot, Subtype=Name(f"/{k}"), Rect=Array([0, 0, 1, 1]))
        )
        for k in ("RichMedia", "Screen", "Sound", "Movie")
    ]
    pdf.pages[0].Annots = Array(annots)
    assert kinds(pdf_active_findings(pdf)) == {"Embedded media": "activeContent"}
    assert remove_active_content(pdf) == {"media": 4}
    assert len(pdf.pages[0].Annots) == 0


def test_plain_pdf_has_no_findings(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.save(tmp_path / "p.pdf")
    assert active_findings(tmp_path / "p.pdf", "pdf") == []


# ----------------------------------------------------------------- OOXML


def write_zip(path: Path, parts: dict[str, bytes | str]) -> Path:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return path


def rels(*items: tuple[str, str, str, bool]) -> str:
    body = "".join(
        f'<Relationship Id="{i}" Type="{REL}/{t}" Target="{target}"'
        + (' TargetMode="External"' if ext else "")
        + "/>"
        for i, t, target, ext in items
    )
    return f'<?xml version="1.0"?><Relationships xmlns="{PKG}">{body}</Relationships>'


def docm(path: Path, document_body: str = "<w:p><w:r><w:t>Hi</w:t></w:r></w:p>") -> Path:
    content_types = (
        f'<?xml version="1.0"?><Types xmlns="{CT}">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/>'
        f'<Override PartName="/word/document.xml" ContentType="{MAIN_DOCM}"/>'
        '<Override PartName="/word/vbaData.xml" ContentType="application/vnd.ms-word.vbaData+xml"/>'
        "</Types>"
    )
    document = (
        f'<?xml version="1.0"?><w:document xmlns:w="{W}" xmlns:r="{REL}"><w:body>'
        f"{document_body}</w:body></w:document>"
    )
    return write_zip(
        path,
        {
            "[Content_Types].xml": content_types,
            "_rels/.rels": rels(("rId1", "officeDocument", "word/document.xml", False)),
            "word/document.xml": document,
            "word/_rels/document.xml.rels": rels(
                ("rId1", "vbaProject", "vbaProject.bin", False),
                ("rId2", "hyperlink", "https://example.com/", True),
            ),
            "word/vbaProject.bin": b"\xd0\xcf\x11\xe0 macro",
            "word/vbaData.xml": f'<wne:vbaSuppressData xmlns:wne="{W}"/>',
        },
    )


def test_docm_is_disarmed_to_a_plain_docx(tmp_path: Path) -> None:
    src = docm(tmp_path / "a.docm")
    assert kinds(active_findings(src, "docx"))["Macros (VBA)"] == "macros"
    counts = disarm_ooxml(src, tmp_path / "a.docx")
    assert counts["macros"] == 2
    assert active_findings(tmp_path / "a.docx", "docx") == []
    with zipfile.ZipFile(tmp_path / "a.docx") as z:
        assert z.testzip() is None
        names = set(z.namelist())
        assert "word/vbaProject.bin" not in names and "word/vbaData.xml" not in names
        types = etree.fromstring(z.read("[Content_Types].xml"))
        values = [e.get("ContentType") for e in types]
        assert MAIN_DOCX in values and MAIN_DOCM not in values
        assert all("vba" not in v for v in values)
        rel = etree.fromstring(z.read("word/_rels/document.xml.rels"))
        assert [r.get("Id") for r in rel] == ["rId2"]  # the web link stays
        etree.fromstring(z.read("word/document.xml"))


def test_remove_links_drops_web_hyperlinks_and_their_elements(tmp_path: Path) -> None:
    body = '<w:p><w:hyperlink r:id="rId2"><w:r><w:t>site</w:t></w:r></w:hyperlink></w:p>'
    src = docm(tmp_path / "a.docm", body)
    counts = disarm_ooxml(src, tmp_path / "a.docx", remove_links=True)
    assert counts["links"] == 1
    with zipfile.ZipFile(tmp_path / "a.docx") as z:
        assert [r.get("Id") for r in etree.fromstring(z.read("word/_rels/document.xml.rels"))] == []
        doc = z.read("word/document.xml").decode()
        assert "rId2" not in doc and "site" in doc


def test_external_attached_template_and_image(tmp_path: Path) -> None:
    body = '<w:p><w:r><w:t>x</w:t></w:r></w:p><w:settings><w:attachedTemplate r:id="rId9"/></w:settings>'
    src = write_zip(
        tmp_path / "a.docx",
        {
            "[Content_Types].xml": f'<Types xmlns="{CT}"><Override PartName="/word/document.xml" ContentType="{MAIN_DOCX}"/></Types>',
            "word/document.xml": f'<w:document xmlns:w="{W}" xmlns:r="{REL}"><w:body>{body}</w:body></w:document>',
            "word/_rels/document.xml.rels": rels(
                ("rId9", "attachedTemplate", "file:///\\\\server\\share\\t.dotm", True),
                ("rId3", "image", "http://tracker.example/p.png", True),
                ("rId4", "hyperlink", "file:///c:/windows/system32/calc.exe", True),
            ),
        },
    )
    found = kinds(active_findings(src, "docx"))
    assert found["Loads 3 things from outside the file"] == "externalLinks"
    counts = disarm_ooxml(src, tmp_path / "b.docx")
    assert counts["external"] == 3
    assert active_findings(tmp_path / "b.docx", "docx") == []
    with zipfile.ZipFile(tmp_path / "b.docx") as z:
        assert "attachedTemplate" not in z.read("word/document.xml").decode()


def test_dde_fields_and_embeddings_and_activex(tmp_path: Path) -> None:
    body = (
        '<w:p><w:fldSimple w:instr="DDEAUTO c:\\\\windows\\\\system32\\\\cmd.exe"><w:r><w:t>r</w:t></w:r>'
        "</w:fldSimple></w:p>"
        '<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> DD</w:instrText></w:r>'
        '<w:r><w:instrText>E "cmd"</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>'
        '<w:r><w:t>x</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>'
        '<w:p><w:r><w:object><w:control r:id="rId5"/></w:object></w:r></w:p>'
    )
    src = write_zip(
        tmp_path / "a.docx",
        {
            "[Content_Types].xml": f'<Types xmlns="{CT}"><Override PartName="/word/document.xml" ContentType="{MAIN_DOCX}"/></Types>',
            "word/document.xml": f'<w:document xmlns:w="{W}" xmlns:r="{REL}"><w:body>{body}</w:body></w:document>',
            "word/_rels/document.xml.rels": rels(
                ("rId5", "control", "activeX/activeX1.xml", False),
                ("rId6", "oleObject", "embeddings/oleObject1.bin", False),
                ("rId7", "package", "embeddings/Microsoft_Excel_Sheet1.xlsx", False),
            ),
            "word/activeX/activeX1.xml": "<ocx/>",
            "word/activeX/activeX1.bin": b"bin",
            "word/activeX/_rels/activeX1.xml.rels": rels(("rId1", "ocxPr", "activeX1.bin", False)),
            "word/embeddings/oleObject1.bin": b"ole",
            "word/embeddings/Microsoft_Excel_Sheet1.xlsx": b"PK",
        },
    )
    found = kinds(active_findings(src, "docx"))
    assert found["Automatic links (DDE)"] == "activeContent"
    assert found["ActiveX controls"] == "activeContent"
    assert found["2 embedded files"] == "embeddedFiles"
    counts = disarm_ooxml(src, tmp_path / "b.docx")
    assert counts["dde"] == 2 and counts["activeX"] == 2 and counts["embedded"] == 2
    assert active_findings(tmp_path / "b.docx", "docx") == []
    with zipfile.ZipFile(tmp_path / "b.docx") as z:
        assert not [n for n in z.namelist() if "activeX" in n or "embeddings" in n]
        assert b"DDE" not in z.read("word/document.xml")


def test_xlsx_external_workbook_links(tmp_path: Path) -> None:
    sheet_ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
    src = write_zip(
        tmp_path / "a.xlsx",
        {
            "[Content_Types].xml": f'<Types xmlns="{CT}"><Override PartName="/xl/workbook.xml" ContentType="x"/></Types>',
            "xl/workbook.xml": (
                f'<workbook xmlns="{sheet_ns}" xmlns:r="{REL}"><sheets/>'
                '<externalReferences><externalReference r:id="rId7"/></externalReferences></workbook>'
            ),
            "xl/_rels/workbook.xml.rels": rels(
                ("rId7", "externalLink", "externalLinks/externalLink1.xml", False)
            ),
            "xl/externalLinks/externalLink1.xml": "<externalLink/>",
            "xl/externalLinks/_rels/externalLink1.xml.rels": rels(
                ("rId1", "externalLinkPath", "file:///c:/other.xlsx", True)
            ),
        },
    )
    assert kinds(active_findings(src, "xlsx")) == {"Links to other workbooks": "externalLinks"}
    disarm_ooxml(src, tmp_path / "b.xlsx")
    assert active_findings(tmp_path / "b.xlsx", "xlsx") == []
    with zipfile.ZipFile(tmp_path / "b.xlsx") as z:
        book = z.read("xl/workbook.xml").decode()
        assert "externalReference" not in book


def test_pptx_program_action_and_ole_frame(tmp_path: Path) -> None:
    a = "http://schemas.openxmlformats.org/drawingml/2006/main"
    p = "http://schemas.openxmlformats.org/presentationml/2006/main"
    slide = (
        f'<p:sld xmlns:p="{p}" xmlns:a="{a}" xmlns:r="{REL}"><p:cSld><p:spTree>'
        '<p:sp><p:nvSpPr><p:cNvPr id="2" name="b"><a:hlinkClick r:id="rId2" action="ppaction://program"/>'
        "</p:cNvPr></p:nvSpPr></p:sp>"
        '<p:graphicFrame><a:graphic><a:graphicData><p:oleObj r:id="rId3"/></a:graphicData></a:graphic>'
        "</p:graphicFrame></p:spTree></p:cSld></p:sld>"
    )
    src = write_zip(
        tmp_path / "a.pptx",
        {
            "[Content_Types].xml": f'<Types xmlns="{CT}"/>',
            "ppt/slides/slide1.xml": slide,
            "ppt/slides/_rels/slide1.xml.rels": rels(
                ("rId2", "hyperlink", "calc.exe", True),
                ("rId3", "oleObject", "../embeddings/o.bin", False),
            ),
            "ppt/embeddings/o.bin": b"ole",
        },
    )
    found = kinds(active_findings(src, "pptx"))
    assert "Runs a program or macro" in found and "1 embedded file" in found
    disarm_ooxml(src, tmp_path / "b.pptx")
    assert active_findings(tmp_path / "b.pptx", "pptx") == []
    with zipfile.ZipFile(tmp_path / "b.pptx") as z:
        xml = z.read("ppt/slides/slide1.xml").decode()
        assert "oleObj" not in xml and "graphicFrame" not in xml and "hlinkClick" not in xml


def test_ooxml_refuses_hostile_packages(tmp_path: Path) -> None:
    doctype = write_zip(
        tmp_path / "d.docx",
        {
            "[Content_Types].xml": f'<Types xmlns="{CT}"/>',
            "word/document.xml": '<!DOCTYPE x [<!ENTITY e "x">]><w:document xmlns:w="a">&e;</w:document>',
            "word/_rels/document.xml.rels": rels(("rId1", "image", "http://x/y.png", True)),
        },
    )
    with pytest.raises(AdapterError) as e:
        disarm_ooxml(doctype, tmp_path / "o.docx")
    assert e.value.code == "E_INPUT_CORRUPT"
    notzip = tmp_path / "n.docx"
    notzip.write_bytes(b"not a zip")
    with pytest.raises(AdapterError) as e:
        active_findings(notzip, "docx")
    assert e.value.code == "E_INPUT_CORRUPT"
    encrypted = tmp_path / "e.docx"
    encrypted.write_bytes(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" + b"\0" * 64)
    with pytest.raises(AdapterError) as e:
        active_findings(encrypted, "docx")
    assert e.value.code == "E_PASSWORD_REQUIRED"


def test_check_zip_limits(tmp_path: Path) -> None:
    path = write_zip(tmp_path / "z.zip", {"a": b"x" * 5000, "b": b"y" * 5000})
    with zipfile.ZipFile(path) as zf:
        assert len(check_zip(zf, member_max=10_000, total_max=20_000)) == 2
        with pytest.raises(AdapterError) as e:
            check_zip(zf, member_max=4000, total_max=20_000)
        assert e.value.code == "E_LIMIT_EXCEEDED"
        with pytest.raises(AdapterError):
            check_zip(zf, member_max=10_000, total_max=9_000)
    bomb = write_zip(tmp_path / "bomb.zip", {"a": b"\0" * 20_000_000})
    with zipfile.ZipFile(bomb) as zf:
        with pytest.raises(AdapterError) as e:
            check_zip(zf, member_max=10**9, total_max=10**9)
        assert e.value.detail["limit"] == "ratio"


def test_parse_xml_refuses_doctype() -> None:
    with pytest.raises(AdapterError):
        parse_xml(b'<!DOCTYPE a [<!ENTITY e "x">]><a/>')
    assert parse_xml(b"<a/>").tag == "a"


# ------------------------------------------------------------- SVG, HTML

SVG = """<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" onload="x()"
     width="10" height="10">
  <script>alert(1)</script>
  <style>@import url(http://evil.example/a.css); .a { fill: url(#g); background: url(http://t/p.png) }</style>
  <rect id="r" width="5" height="5" onclick="x()" style="fill:url(#g)"/>
  <image xlink:href="http://example.com/a.png" width="5" height="5"/>
  <image href="data:image/png;base64,AAAA" width="5" height="5"/>
  <use xlink:href="#r"/>
  <a xlink:href="javascript:alert(1)"><text>x</text></a>
  <a xlink:href="https://example.com/"><text>site</text></a>
  <foreignObject><div xmlns="http://www.w3.org/1999/xhtml">hi</div></foreignObject>
</svg>"""


def test_svg_is_disarmed(tmp_path: Path) -> None:
    src = tmp_path / "a.svg"
    src.write_text(SVG)
    found = kinds(active_findings(src, "svg"))
    assert found["JavaScript"] == "activeContent"
    assert found["Embedded frames or objects"] == "activeContent"
    assert found["Loads 3 things from outside the file"] == "externalLinks"
    counts = disarm_markup(src, tmp_path / "b.svg", "svg")
    assert counts["script"] == 1 and counts["handler"] == 2 and counts["jsUrl"] == 1
    assert active_findings(tmp_path / "b.svg", "svg") == []
    out = etree.parse(str(tmp_path / "b.svg")).getroot()
    text = (tmp_path / "b.svg").read_text()
    assert "http://evil" not in text and "tracker" not in text and "<script" not in text
    assert "data:image/png" in text and 'href="https://example.com/"' in text
    assert "#g" in text and "#r" in text  # fragments stay
    assert out.tag.endswith("svg")


def test_svg_remove_links_drops_web_hrefs(tmp_path: Path) -> None:
    src = tmp_path / "a.svg"
    src.write_text(SVG)
    counts = disarm_markup(src, tmp_path / "b.svg", "svg", remove_links=True)
    assert counts["links"] == 1
    assert 'href="https://example.com/"' not in (tmp_path / "b.svg").read_text()


def test_svg_with_doctype_is_refused(tmp_path: Path) -> None:
    src = tmp_path / "a.svg"
    src.write_text(
        '<!DOCTYPE svg [<!ENTITY e "x">]><svg xmlns="http://www.w3.org/2000/svg">&e;</svg>'
    )
    with pytest.raises(AdapterError) as e:
        active_findings(src, "svg")
    assert e.value.code == "E_INPUT_CORRUPT"


HTML = """<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>t</title>
<meta http-equiv="refresh" content="0;url=http://evil.example/">
<base href="http://evil.example/">
<link rel="stylesheet" href="http://cdn.example/a.css"><link rel="canonical" href="http://x/">
<script src="http://cdn.example/a.js"></script>
<style>body { background: url(http://t/p.png) } p { color: red }</style>
</head><body onload="x()">
<p onclick="x()">Caf\u00e9 <a href="https://example.com/">site</a> <a href="#top">top</a></p>
<a href="javascript:alert(1)">bad</a>
<img src="http://tracker.example/p.gif"><img src="data:image/png;base64,AAAA">
<iframe src="http://evil.example/"></iframe>
<object data="x.swf"></object><embed src="x.swf">
<script>alert(1)</script>
<!--[if IE]><script>alert(2)</script><![endif]-->
</body></html>"""


def test_html_is_disarmed(tmp_path: Path) -> None:
    src = tmp_path / "a.html"
    src.write_text(HTML, encoding="utf-8")
    found = kinds(active_findings(src, "html"))
    assert found["JavaScript"] == "activeContent"
    assert found["Embedded frames or objects"] == "activeContent"
    assert found["Automatic redirect"] == "activeContent"
    assert found["Changes where links point"] == "activeContent"
    assert "externalLinks" in found.values()
    disarm_markup(src, tmp_path / "b.html", "html")
    assert active_findings(tmp_path / "b.html", "html") == []
    text = (tmp_path / "b.html").read_text("utf-8")
    assert "<script" not in text and "<iframe" not in text and "evil.example" not in text
    assert "tracker.example" not in text and "cdn.example" not in text
    assert 'href="https://example.com/"' in text and 'href="#top"' in text
    assert "Caf\u00e9" in text and "data:image/png" in text and "canonical" in text
    assert "color: red" in text


def test_other_formats_have_no_findings(tmp_path: Path) -> None:
    f = tmp_path / "a.png"
    f.write_bytes(b"x")
    assert active_findings(f, "png") == []
