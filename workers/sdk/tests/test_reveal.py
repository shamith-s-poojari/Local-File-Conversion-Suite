"""What a file reveals (converter_sdk.reveal)."""

from __future__ import annotations

import io
import json
import subprocess
import zipfile
from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import context
from converter_sdk import reveal
from converter_sdk.errors import AdapterError
from converter_sdk.protocol import Finding
from PIL import Image

NO_FFMPEG = files.ffmpeg() is None


def kinds(found: list[Finding]) -> set[str]:
    return {f.kind for f in found}


def details(found: list[Finding], kind: str) -> list[str]:
    return [f.detail or "" for f in found if f.kind == kind]


def ctx_for(tmp_path: Path):
    (tmp_path / "job").mkdir(exist_ok=True)
    ctx, _ = context(tmp_path / "job", [])
    return ctx


# ------------------------------------------------------------------ images


def exif_jpeg(path: Path) -> Path:
    exif = Image.Exif()
    exif[0x010F] = "Acme"
    exif[0x0110] = "Acme X100"
    exif[0x0131] = "Editor 4.2"
    exif[0x0132] = "2024:05:01 10:00:00"
    exif[0x013B] = "Ada Lovelace"
    exif[0x8298] = "(c) Ada"
    exif.get_ifd(0x8825).update({1: "N", 2: (37.0, 46.0, 29.64), 3: "W", 4: (122.0, 25.0, 9.84)})
    exif.get_ifd(0x8769).update({0x9003: "2024:05:01 09:59:00", 0xA431: "SN123", 0xA434: "Lens 50"})
    Image.new("RGB", (16, 16), (200, 30, 30)).save(path, "JPEG", exif=exif.tobytes())
    return path


def test_jpeg_exif_findings(tmp_path: Path) -> None:
    found = reveal.image_findings(exif_jpeg(tmp_path / "a.jpg"))
    assert details(found, "location") == ["37.7749, -122.4194"]
    assert "Acme X100" in details(found, "device")
    assert "Camera serial number SN123" in details(found, "device")
    assert "Lens Lens 50" in details(found, "device")
    assert details(found, "software") == ["Editor 4.2"]
    assert "2024-05-01 09:59:00" in details(found, "dates")
    assert set(details(found, "author")) == {"Ada Lovelace", "(c) Ada"}


def test_clean_image_has_no_findings(tmp_path: Path) -> None:
    p = tmp_path / "plain.png"
    Image.new("RGB", (8, 8)).save(p)
    assert reveal.image_findings(p) == []


def test_jpeg_comment_never_quoted(tmp_path: Path) -> None:
    p = tmp_path / "c.jpg"
    Image.new("RGB", (8, 8)).save(p, "JPEG", comment=b"my secret note")
    found = reveal.image_findings(p)
    assert details(found, "comments") == ["Embedded comment"]
    assert "secret" not in json.dumps([f.model_dump() for f in found])


def test_png_text_chunks(tmp_path: Path) -> None:
    from PIL.PngImagePlugin import PngInfo

    info = PngInfo()
    info.add_text("Author", "Grace")
    info.add_text("Software", "Painter")
    info.add_text("Description", "a long private description")
    info.add_text("Comment", "private")
    p = tmp_path / "t.png"
    Image.new("RGB", (8, 8)).save(p, pnginfo=info)
    found = reveal.image_findings(p)
    assert details(found, "author") == ["Grace"]
    assert details(found, "software") == ["Painter"]
    assert details(found, "comments") == ["Embedded comment"]
    assert details(found, "tags") == ["Text note: Description"]
    assert "private" not in json.dumps([f.model_dump() for f in found])


def test_jpeg_xmp_is_tags(tmp_path: Path) -> None:
    p = tmp_path / "x.png"
    from PIL.PngImagePlugin import PngInfo

    info = PngInfo()
    info.add_itxt("XML:com.adobe.xmp", "<x:xmpmeta xmlns:x='adobe:ns:meta/'/>")
    Image.new("RGB", (8, 8)).save(p, pnginfo=info)
    assert "XMP metadata" in details(reveal.image_findings(p), "tags")


def test_image_encode_drops_source_comment(tmp_path: Path) -> None:
    from converter_sdk.images import Frames, encode

    img = Image.new("RGB", (8, 8))
    img.info["comment"] = b"leaky comment"
    for fmt in ("jpg", "gif"):
        data = encode(Frames([img]), fmt)
        assert b"leaky comment" not in data
    two = [Image.new("RGB", (8, 8)), Image.new("RGB", (8, 8))]
    for frame in two:
        frame.info["comment"] = b"leaky comment"
    assert b"leaky comment" not in encode(Frames(two, [100, 100]), "gif")


# --------------------------------------------------------------------- PDF


def test_pdf_findings(tmp_path: Path) -> None:
    import pikepdf

    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.trailer["/Info"] = pdf.make_indirect(
        pikepdf.Dictionary(
            Author=pikepdf.String("Ada"),
            Creator=pikepdf.String("Writer"),
            Producer=pikepdf.String("Press 9"),
            CreationDate=pikepdf.String("D:20240501101500Z"),
            Subject=pikepdf.String("secret subject"),
        )
    )
    note = pdf.make_indirect(
        pikepdf.Dictionary(
            Type=pikepdf.Name.Annot,
            Subtype=pikepdf.Name.Text,
            Rect=[0, 0, 10, 10],
            Contents=pikepdf.String("private note"),
        )
    )
    link = pdf.make_indirect(
        pikepdf.Dictionary(Type=pikepdf.Name.Annot, Subtype=pikepdf.Name.Link, Rect=[0, 0, 1, 1])
    )
    pdf.pages[0].obj["/Annots"] = pikepdf.Array([note, link])
    pdf.Root["/Metadata"] = pdf.make_stream(b"<x/>")
    p = tmp_path / "a.pdf"
    pdf.save(p)
    found = reveal.pdf_findings(pdf)
    assert details(found, "author") == ["Ada"]
    assert set(details(found, "software")) == {"Writer", "Press 9"}
    assert details(found, "dates") == ["2024-05-01 10:15:00"]
    assert details(found, "comments") == ["1 annotation"]
    assert "XMP metadata" in details(found, "tags")
    assert "private" not in json.dumps([f.model_dump() for f in found])
    via_ctx = reveal.findings(ctx_for(tmp_path), p, include_active=False)
    assert kinds(via_ctx) == kinds(found)


def test_pdf_hidden_layers(tmp_path: Path) -> None:
    pikepdf = pytest.importorskip("pikepdf")
    pytest.importorskip("converter_sdk.pdf")
    from converter_sdk import pdf as sdkpdf

    if not hasattr(sdkpdf, "hidden_ocgs"):
        pytest.skip("hidden_ocgs is not in converter_sdk.pdf yet")
    pdf = pikepdf.new()
    pdf.add_blank_page()
    ocg = pdf.make_indirect(pikepdf.Dictionary(Type=pikepdf.Name.OCG, Name=pikepdf.String("L")))
    pdf.Root["/OCProperties"] = pikepdf.Dictionary(
        OCGs=pikepdf.Array([ocg]), D=pikepdf.Dictionary(OFF=pikepdf.Array([ocg]))
    )
    assert details(reveal.pdf_findings(pdf), "hiddenContent") == ["1 hidden layer"]


# ------------------------------------------------------------------ OOXML

CORE = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/'
    'core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" '
    'xmlns:dcterms="http://purl.org/dc/terms/" '
    'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
    "<dc:title>Secret plans</dc:title><dc:creator>Ada</dc:creator>"
    "<cp:lastModifiedBy>Bob</cp:lastModifiedBy><cp:revision>7</cp:revision>"
    '<dcterms:created xsi:type="dcterms:W3CDTF">2024-05-01T10:00:00Z</dcterms:created>'
    '<dcterms:modified xsi:type="dcterms:W3CDTF">2024-05-02T10:00:00Z</dcterms:modified>'
    "</cp:coreProperties>"
)
APP = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">'
    "<Application>Word</Application><AppVersion>16.0</AppVersion><Company>Acme</Company>"
    "<Manager>Eve</Manager><Template>Normal.dotm</Template><TotalTime>12</TotalTime>"
    "<Pages>1</Pages></Properties>"
)
CUSTOM = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties">'
    '<property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="Project">'
    '<lpwstr xmlns="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">X'
    "</lpwstr></property></Properties>"
)
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'


def office(path: Path, parts: dict[str, str]) -> Path:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts.items():
            z.writestr(name, text)
    return path


def docx(path: Path, body: str = "<w:p><w:r><w:t>Hello</w:t></w:r></w:p>") -> Path:
    return office(
        path,
        {
            "docProps/core.xml": CORE,
            "docProps/app.xml": APP,
            "docProps/custom.xml": CUSTOM,
            "word/document.xml": f"<w:document {W}><w:body>{body}</w:body></w:document>",
            "word/comments.xml": f'<w:comments {W}><w:comment w:id="0" w:author="Z">'
            "<w:p><w:r><w:t>private remark</w:t></w:r></w:p></w:comment></w:comments>",
        },
    )


def test_docx_properties_and_comments(tmp_path: Path) -> None:
    found = reveal.ooxml_findings(docx(tmp_path / "a.docx"))
    assert set(details(found, "author")) == {"Ada", "Bob", "Acme", "Eve"}
    assert set(details(found, "dates")) == {"2024-05-01T10:00:00Z", "2024-05-02T10:00:00Z"}
    assert details(found, "software") == ["Word 16.0"]
    assert details(found, "tags") == ["Template", "1 custom property"]
    assert details(found, "comments") == ["1 comment"]
    dump = json.dumps([f.model_dump() for f in found])
    assert "Secret plans" not in dump
    assert "private remark" not in dump


@pytest.mark.parametrize(
    ("body", "kind"),
    [
        ('<w:ins w:id="1" w:author="A"><w:r><w:t>x</w:t></w:r></w:ins>', "trackedChanges"),
        (
            '<w:del w:id="1" w:author="A"><w:r><w:delText>x</w:delText></w:r></w:del>',
            "trackedChanges",
        ),
        ("<w:p><w:r><w:rPr><w:vanish/></w:rPr><w:t>x</w:t></w:r></w:p>", "hiddenContent"),
    ],
)
def test_docx_history_and_hidden_text(tmp_path: Path, body: str, kind: str) -> None:
    assert kind in kinds(reveal.ooxml_findings(docx(tmp_path / "b.docx", body)))


def test_docx_without_history_has_none(tmp_path: Path) -> None:
    found = reveal.ooxml_findings(docx(tmp_path / "c.docx"))
    assert not kinds(found) & {"trackedChanges", "hiddenContent"}


def test_xlsx_hidden_sheets(tmp_path: Path) -> None:
    p = office(
        tmp_path / "a.xlsx",
        {
            "xl/workbook.xml": '<workbook xmlns="http://schemas.openxmlformats.org/'
            'spreadsheetml/2006/main"><sheets><sheet name="A" sheetId="1"/>'
            '<sheet name="B" sheetId="2" state="hidden"/>'
            '<sheet name="C" sheetId="3" state="veryHidden"/></sheets></workbook>',
            "xl/comments1.xml": '<comments><commentList><comment ref="A1" authorId="0"/>'
            '<comment ref="A2" authorId="0"/></commentList></comments>',
        },
    )
    found = reveal.ooxml_findings(p)
    assert details(found, "hiddenContent") == ["2 hidden sheets"]
    assert details(found, "comments") == ["2 comments"]


def test_pptx_hidden_slide(tmp_path: Path) -> None:
    ns = 'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
    p = office(
        tmp_path / "a.pptx",
        {
            "ppt/slides/slide1.xml": f"<p:sld {ns}/>",
            "ppt/slides/slide2.xml": f'<p:sld {ns} show="0"/>',
        },
    )
    assert details(reveal.ooxml_findings(p), "hiddenContent") == ["1 hidden slide"]


def test_ooxml_rejects_doctype_and_bad_zip(tmp_path: Path) -> None:
    bad = office(tmp_path / "d.docx", {"docProps/core.xml": '<!DOCTYPE x [<!ENTITY a "b">]><x/>'})
    with pytest.raises(AdapterError) as e:
        reveal.ooxml_findings(bad)
    assert e.value.code == "E_INPUT_CORRUPT"
    notzip = tmp_path / "e.docx"
    notzip.write_bytes(b"not a zip")
    with pytest.raises(AdapterError):
        reveal.ooxml_findings(notzip)


# ------------------------------------------------------------------ markup


def test_svg_and_html(tmp_path: Path) -> None:
    svg = tmp_path / "a.svg"
    svg.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" '
        'xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" '
        'xmlns:sodipodi="x" inkscape:version="1.3" sodipodi:docname="/home/ada/secret.svg">'
        "<metadata><rdf>who</rdf></metadata></svg>"
    )
    found = reveal.markup_findings(svg, "svg")
    assert "Metadata block" in details(found, "tags")
    assert "Editor file names" in details(found, "tags")
    assert details(found, "software") == ["Inkscape 1.3"]
    assert "secret.svg" not in json.dumps([f.model_dump() for f in found])
    page = tmp_path / "a.html"
    page.write_text(
        '<html><head><meta name="Author" content="Ada">'
        '<meta name="generator" content="Hugo 0.1"></head></html>'
    )
    found = reveal.markup_findings(page, "html")
    assert details(found, "author") == ["Ada"]
    assert details(found, "software") == ["Hugo 0.1"]


# ------------------------------------------------------------------- media


def test_media_findings_from_probe_json() -> None:
    info = {
        "format": {
            "tags": {
                "location": "+37.7749-122.4194+010.0/",
                "com.apple.quicktime.make": "Apple",
                "com.apple.quicktime.model": "iPhone 15",
                "encoder": "Lavf61.1.100",
                "creation_time": "2024-05-01T10:00:00Z",
                "ARTIST": "Ada",
                "title": "T",
                "Comment": "c",
            }
        },
        "streams": [
            {"codec_type": "audio", "tags": {"encoder": "Lavc61.3.100 aac"}},
            {"codec_type": "video", "disposition": {"attached_pic": 1}},
        ],
    }
    found = reveal.media_findings(info)
    assert details(found, "location") == ["37.7749, -122.4194"]
    assert details(found, "device") == ["Apple iPhone 15"]
    assert details(found, "software") == []
    assert details(found, "dates") == ["2024-05-01T10:00:00Z"]
    assert details(found, "author") == ["Ada"]
    assert sorted(details(found, "tags")) == ["Cover picture", "Title, Comment"]


def test_media_software_is_reported_unless_lavf() -> None:
    info = {"format": {"tags": {"encoder": "HandBrake 1.7"}}, "streams": []}
    assert details(reveal.media_findings(info), "software") == ["HandBrake 1.7"]


@pytest.mark.skipif(NO_FFMPEG, reason="needs ffmpeg")
def test_mp4_with_location(tmp_path: Path) -> None:
    from converter_sdk.media import probe

    out = tmp_path / "loc.mp4"
    subprocess.run(  # noqa: S603
        [
            str(files.ffmpeg()), "-v", "error", "-y", "-f", "lavfi",
            "-i", "color=c=red:s=64x64:d=1",
            "-metadata", "location=+37.7749-122.4194/", "-metadata", "artist=Ada",
            "-pix_fmt", "yuv420p", str(out),
        ],
        check=True,
        capture_output=True,
    )  # fmt: skip
    ctx = ctx_for(tmp_path)
    found = reveal.media_findings(probe(ctx, out))
    assert details(found, "location") == ["37.7749, -122.4194"]
    assert "Ada" in details(found, "author")
    assert kinds(reveal.findings(ctx, out)) >= {"location", "author"}


# ---------------------------------------------------------------- dispatch


def test_findings_caps_dedups_and_sanitizes(tmp_path: Path) -> None:
    assert reveal.clean_detail("a\x00b\n\tc " + "x" * 300) == ("a b c " + "x" * 300)[:200]
    many = [Finding(kind="tags", detail=str(i % 80)) for i in range(300)]
    assert len(reveal._tidy(many)) == 64
    assert len(reveal._tidy([Finding(kind="tags", detail="a")] * 5)) == 1


def test_findings_dispatch_by_format(tmp_path: Path) -> None:
    ctx = ctx_for(tmp_path)
    jpg = exif_jpeg(tmp_path / "x.jpg")
    assert "location" in kinds(reveal.findings(ctx, jpg, include_active=False))
    d = docx(tmp_path / "x.docx")
    assert "comments" in kinds(reveal.findings(ctx, d, include_active=False))
    other = tmp_path / "x.txt"
    other.write_text("hello")
    assert reveal.findings(ctx, other) == []


def test_png_bytes_are_unused_here(tmp_path: Path) -> None:
    # A plain image round-trips through the dispatcher without findings.
    buf = io.BytesIO()
    Image.new("RGB", (4, 4)).save(buf, "PNG")
    p = tmp_path / "q.png"
    p.write_bytes(buf.getvalue())
    assert reveal.findings(ctx_for(tmp_path), p, include_active=False) == []
