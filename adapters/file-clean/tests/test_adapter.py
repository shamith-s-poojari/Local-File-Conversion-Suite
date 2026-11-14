"""Acceptance tests for file-clean: hidden details come out, pictures keep their pixels."""

from __future__ import annotations

import io
import zipfile
from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import context, load_adapter_class, make_job_dir, run_in_process
from converter_sdk import reveal
from converter_sdk.errors import AdapterError
from PIL import Image
from PIL.PngImagePlugin import PngInfo

ADAPTER = Path(__file__).resolve().parents[1]
BAD = {"location", "device", "author", "dates"}


def exif_bytes(orientation: int = 1) -> bytes:
    exif = Image.Exif()
    exif[0x0112] = orientation
    exif[0x010F] = "Acme"
    exif[0x0110] = "Acme X100"
    exif[0x0131] = "Editor 4.2"
    exif[0x0132] = "2024:05:01 10:00:00"
    exif[0x013B] = "Ada Lovelace"
    exif.get_ifd(0x8825).update({1: "N", 2: (37.0, 46.0, 29.64), 3: "W", 4: (122.0, 25.0, 9.84)})
    exif.get_ifd(0x8769).update({0x9003: "2024:05:01 09:59:00"})
    return exif.tobytes()


def picture(size: tuple[int, int] = (64, 48)) -> Image.Image:
    img = Image.new("RGB", size)
    for x in range(size[0]):
        for y in range(size[1]):
            img.putpixel((x, y), ((x * 7) % 256, (y * 5) % 256, (x * y) % 256))
    return img


def ctx_for(tmp_path: Path):
    (tmp_path / "job").mkdir(exist_ok=True)
    return context(tmp_path / "job", [])[0]


def run(tmp_path: Path, src: Path, params: dict | None = None):
    r = run_in_process(ADAPTER, tmp_path / "job", [src], params or {})
    assert not r.errors
    return r


def leftovers(tmp_path: Path, path: Path) -> set[str]:
    return {f.kind for f in reveal.findings(ctx_for(tmp_path), path, include_active=False)}


def pixels_of(path: Path) -> list[tuple[int, ...]]:
    with Image.open(path) as img:
        return list(img.convert("RGBA").getdata())


# --------------------------------------------------------------- pictures


def test_jpeg_is_cleaned_without_reencoding(tmp_path: Path) -> None:
    src = tmp_path / "in.jpg"
    picture().save(src, "JPEG", exif=exif_bytes(), comment=b"private note", quality=90)
    assert BAD <= leftovers(tmp_path, src)
    r = run(tmp_path, src, {"fileTitles": ["trip"]})
    assert r.names == ["trip-clean.jpg"]
    out = r.outputs[0]
    assert not leftovers(tmp_path, out) & (BAD | {"comments"})
    assert b"private note" not in out.read_bytes()
    assert b"Exif" not in out.read_bytes()
    assert pixels_of(src) == pixels_of(out)
    assert "DETAILS_REMOVED" in r.warnings
    assert "REENCODED" not in r.warnings
    # The compressed data is the very same bytes.
    assert (
        src.read_bytes()
        .split(b"\xff\xda")[-1]
        .startswith(out.read_bytes().split(b"\xff\xda")[-1][:50])
    )


def test_jpeg_keeps_icc_profile(tmp_path: Path) -> None:
    src = tmp_path / "in.jpg"
    profile = Image.open(io.BytesIO(_srgb())).info["icc_profile"]
    picture().save(src, "JPEG", exif=exif_bytes(), icc_profile=profile)
    out = run(tmp_path, src).outputs[0]
    with Image.open(out) as img:
        assert img.info.get("icc_profile") == profile


def _srgb() -> bytes:
    from PIL import ImageCms

    buf = io.BytesIO()
    prof = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    Image.new("RGB", (1, 1)).save(buf, "PNG", icc_profile=prof)
    return buf.getvalue()


def test_sideways_jpeg_is_turned_upright_and_warns(tmp_path: Path) -> None:
    src = tmp_path / "in.jpg"
    picture((64, 48)).save(src, "JPEG", exif=exif_bytes(6), quality=95)
    r = run(tmp_path, src)
    assert "REENCODED" in r.warnings
    with Image.open(r.outputs[0]) as img:
        assert img.size == (48, 64)
        assert not img.getexif().get(0x0112)
    assert not leftovers(tmp_path, r.outputs[0]) & BAD


def test_png_text_chunks_are_dropped_and_pixels_kept(tmp_path: Path) -> None:
    info = PngInfo()
    info.add_text("Author", "Grace")
    info.add_text("Software", "Painter")
    info.add_itxt("XML:com.adobe.xmp", "<x:xmpmeta xmlns:x='adobe:ns:meta/'/>")
    info.add_text("Comment", "private")
    src = tmp_path / "in.png"
    img = picture()
    img.putalpha(200)
    img.save(src, pnginfo=info, exif=exif_bytes())
    assert BAD <= leftovers(tmp_path, src)
    out = run(tmp_path, src).outputs[0]
    data = out.read_bytes()
    for chunk in (b"tEXt", b"iTXt", b"zTXt", b"eXIf", b"tIME"):
        assert chunk not in data
    assert leftovers(tmp_path, out) == set()
    assert pixels_of(src) == pixels_of(out)


def test_webp_exif_and_xmp_are_dropped(tmp_path: Path) -> None:
    src = tmp_path / "in.webp"
    picture().save(
        src, "WEBP", lossless=True, exif=exif_bytes(), xmp=b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>"
    )
    assert BAD <= leftovers(tmp_path, src)
    out = run(tmp_path, src).outputs[0]
    data = out.read_bytes()
    assert b"EXIF" not in data
    assert b"XMP " not in data
    assert int.from_bytes(data[4:8], "little") == len(data) - 8
    assert not leftovers(tmp_path, out)
    assert pixels_of(src) == pixels_of(out)


def test_tiff_is_reencoded_losslessly(tmp_path: Path) -> None:
    src = tmp_path / "in.tiff"
    exif = Image.Exif()
    exif[0x013B] = "Ada"
    exif[0x010F] = "Acme"
    exif[0x0132] = "2024:05:01 10:00:00"
    picture().save(src, "TIFF", tiffinfo=exif)
    assert leftovers(tmp_path, src) & BAD
    r = run(tmp_path, src)
    assert "REENCODED" in r.warnings
    assert not leftovers(tmp_path, r.outputs[0]) & BAD
    assert pixels_of(src) == pixels_of(r.outputs[0])


def test_avif_is_reencoded(tmp_path: Path) -> None:
    src = files.image_file(tmp_path / "in.avif", "avif")
    r = run(tmp_path, src)
    assert "REENCODED" in r.warnings
    with Image.open(r.outputs[0]) as img:
        assert img.format == "AVIF"


def test_clean_picture_is_unchanged_in_content(tmp_path: Path) -> None:
    src = tmp_path / "in.png"
    picture().save(src)
    r = run(tmp_path, src)
    assert "DETAILS_REMOVED" not in r.warnings
    assert pixels_of(src) == pixels_of(r.outputs[0])


@pytest.mark.parametrize("fmt", ["jpg", "png", "webp"])
def test_truncated_pictures_fail_as_corrupt(tmp_path: Path, fmt: str) -> None:
    good = files.image_file(tmp_path / f"good.{fmt}", fmt)
    bad = tmp_path / f"bad.{fmt}"
    bad.write_bytes(good.read_bytes()[:-40])
    try:
        r = run_in_process(ADAPTER, tmp_path / "job", [bad], {})
    except AdapterError as e:
        assert e.code == "E_INPUT_CORRUPT"
    else:
        assert [i.code for i in r.errors] == ["E_INPUT_CORRUPT"]


def test_outputs_name_their_source(tmp_path: Path) -> None:
    a = files.image_file(tmp_path / "a.png", "png")
    b = files.image_file(tmp_path / "b.jpg", "jpg")
    adapter = load_adapter_class(ADAPTER)()
    job, rel = make_job_dir(tmp_path / "job", [a, b])
    ctx, _ = context(job, rel)
    outs = adapter.execute_conversion(ctx, {})
    assert [o.source for o in outs] == [0, 1]


# -------------------------------------------------------------------- PDF


def make_pdf(path: Path) -> Path:
    import pikepdf
    from pikepdf import Dictionary, Name, String

    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.trailer["/Info"] = pdf.make_indirect(
        Dictionary(Author=String("Ada"), Creator=String("W"), CreationDate=String("D:20240501"))
    )
    pdf.Root["/Metadata"] = pdf.make_stream(b"<x/>")
    page = pdf.pages[0].obj
    page["/PieceInfo"] = Dictionary(Foo=Dictionary())
    page["/Thumb"] = pdf.make_stream(b"\x00")
    note = pdf.make_indirect(
        Dictionary(Type=Name.Annot, Subtype=Name.Text, Rect=[0, 0, 5, 5], Contents=String("hi"))
    )
    link = pdf.make_indirect(
        Dictionary(Type=Name.Annot, Subtype=Name.Link, Rect=[0, 0, 1, 1], Border=[0, 0, 0])
    )
    page["/Annots"] = pikepdf.Array([note, link])
    pdf.save(path)
    return path


def test_pdf_metadata_and_comments(tmp_path: Path) -> None:
    import pikepdf

    src = make_pdf(tmp_path / "in.pdf")
    assert leftovers(tmp_path, src) >= {"author", "dates", "comments", "tags"}
    r = run(tmp_path, src)
    assert "DETAILS_REMOVED" in r.warnings
    with pikepdf.open(r.outputs[0]) as pdf:
        assert "/Info" not in pdf.trailer or not len(pdf.trailer["/Info"].keys())
        assert "/Metadata" not in pdf.Root
        page = pdf.pages[0].obj
        assert "/PieceInfo" not in page
        assert "/Thumb" not in page
        assert [str(a.Subtype) for a in page.Annots] == ["/Link"]
    assert not leftovers(tmp_path, r.outputs[0]) & (BAD | {"comments", "tags"})


def test_pdf_keeps_comments_when_asked(tmp_path: Path) -> None:
    src = make_pdf(tmp_path / "in.pdf")
    r = run(tmp_path, src, {"removeComments": False})
    found = leftovers(tmp_path, r.outputs[0])
    assert "comments" in found
    assert not found & BAD


def test_pdf_hidden_layers_option(tmp_path: Path) -> None:
    import pikepdf
    from converter_sdk import pdf as sdkpdf

    if not hasattr(sdkpdf, "remove_hidden_layers"):
        pytest.skip("remove_hidden_layers is not in converter_sdk.pdf yet")
    src = tmp_path / "in.pdf"
    pdf = pikepdf.new()
    pdf.add_blank_page()
    ocg = pdf.make_indirect(pikepdf.Dictionary(Type=pikepdf.Name.OCG, Name=pikepdf.String("L")))
    pdf.Root["/OCProperties"] = pikepdf.Dictionary(
        OCGs=pikepdf.Array([ocg]), D=pikepdf.Dictionary(OFF=pikepdf.Array([ocg]))
    )
    pdf.save(src)
    kept = run(tmp_path, src, {"removeHiddenLayers": False}).outputs[0]
    assert "hiddenContent" in leftovers(tmp_path, kept)
    gone = run(tmp_path / "x", src, {"removeHiddenLayers": True}).outputs[0]
    assert "hiddenContent" not in leftovers(tmp_path, gone)


# ----------------------------------------------------------------- Office

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
CORE = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/'
    'core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" '
    'xmlns:dcterms="http://purl.org/dc/terms/" '
    'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
    "<dc:title>Plans</dc:title><dc:creator>Ada</dc:creator>"
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
    '<property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="P"/></Properties>'
)
CT = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/word/document.xml" ContentType="x"/>'
    '<Override PartName="/word/comments.xml" ContentType="c"/>'
    '<Override PartName="/docProps/custom.xml" ContentType="y"/>'
    "</Types>"
)
ROOT_RELS = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="r1" Type="t/officeDocument" Target="word/document.xml"/>'
    '<Relationship Id="r2" Type="t/custom-properties" Target="docProps/custom.xml"/>'
    '<Relationship Id="r3" Type="t/thumbnail" Target="docProps/thumbnail.jpeg"/>'
    "</Relationships>"
)
DOC_RELS = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="t/comments" Target="comments.xml"/>'
    '<Relationship Id="rId2" Type="t/link" Target="https://example.com" TargetMode="External"/>'
    "</Relationships>"
)


def make_docx(path: Path, body: str = "") -> Path:
    parts = {
        "[Content_Types].xml": CT,
        "_rels/.rels": ROOT_RELS,
        "docProps/core.xml": CORE,
        "docProps/app.xml": APP,
        "docProps/custom.xml": CUSTOM,
        "docProps/thumbnail.jpeg": "jpegbytes",
        "word/_rels/document.xml.rels": DOC_RELS,
        "word/document.xml": f"<w:document {W}><w:body><w:p>"
        '<w:commentRangeStart w:id="0"/><w:r><w:t>Body text</w:t></w:r>'
        '<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r>'
        f"</w:p>{body}</w:body></w:document>",
        "word/comments.xml": f'<w:comments {W}><w:comment w:id="0" w:author="Z"><w:p><w:r>'
        "<w:t>private remark</w:t></w:r></w:p></w:comment></w:comments>",
    }
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts.items():
            z.writestr(name, text)
    return path


def test_docx_properties_comments_and_order(tmp_path: Path) -> None:
    src = make_docx(tmp_path / "in.docx")
    r = run(tmp_path, src, {"fileTitles": ["memo"]})
    assert r.names == ["memo-clean.docx"]
    out = r.outputs[0]
    assert "DETAILS_REMOVED" in r.warnings
    assert "TRACKED_CHANGES_KEPT" not in r.warnings
    with zipfile.ZipFile(src) as zin, zipfile.ZipFile(out) as z:
        assert z.testzip() is None
        kept = [n for n in zin.namelist() if n in z.namelist()]
        assert z.namelist() == kept  # same order
        assert "docProps/custom.xml" not in z.namelist()
        assert "docProps/thumbnail.jpeg" not in z.namelist()
        assert "word/comments.xml" not in z.namelist()
        assert b"comments.xml" not in z.read("[Content_Types].xml")
        assert b"custom.xml" not in z.read("[Content_Types].xml")
        assert b"custom-properties" not in z.read("_rels/.rels")
        assert b"thumbnail" not in z.read("_rels/.rels")
        assert b"comments.xml" not in z.read("word/_rels/document.xml.rels")
        assert b"https://example.com" in z.read("word/_rels/document.xml.rels")
        document = z.read("word/document.xml")
        assert b"Body text" in document
        assert b"comment" not in document
        core = z.read("docProps/core.xml")
        assert b"Ada" not in core
        assert b"Bob" not in core
        assert b"2024" not in core
        assert b"Plans" in core
        app = z.read("docProps/app.xml")
        for gone in (b"Acme", b"Eve", b"Normal.dotm", b"TotalTime", b"16.0"):
            assert gone not in app
        assert b"<Pages>1</Pages>" in app
        assert b"Microsoft Office" in app
    assert not leftovers(tmp_path, out) & (BAD | {"comments", "tags"})


def test_docx_tracked_changes_are_kept_with_a_warning(tmp_path: Path) -> None:
    body = '<w:ins w:id="5" w:author="A"><w:r><w:t>new</w:t></w:r></w:ins>'
    r = run(tmp_path, make_docx(tmp_path / "in.docx", body))
    assert "TRACKED_CHANGES_KEPT" in r.warnings
    with zipfile.ZipFile(r.outputs[0]) as z:
        assert b"<w:ins " in z.read("word/document.xml")


def test_docx_keeps_comments_when_asked(tmp_path: Path) -> None:
    r = run(tmp_path, make_docx(tmp_path / "in.docx"), {"removeComments": False})
    with zipfile.ZipFile(r.outputs[0]) as z:
        assert "word/comments.xml" in z.namelist()
        assert b"commentReference" in z.read("word/document.xml")


def test_xlsx_and_pptx_comments(tmp_path: Path) -> None:
    xlsx = tmp_path / "in.xlsx"
    with zipfile.ZipFile(xlsx, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(
            "[Content_Types].xml", CT.replace("word/", "xl/").replace("document", "workbook")
        )
        z.writestr("docProps/core.xml", CORE)
        z.writestr("docProps/app.xml", APP)
        z.writestr("xl/workbook.xml", "<workbook/>")
        z.writestr("xl/comments1.xml", '<comments><comment ref="A1" authorId="0"/></comments>')
        z.writestr(
            "xl/worksheets/_rels/sheet1.xml.rels",
            DOC_RELS.replace('"comments.xml"', '"../comments1.xml"'),
        )
    r = run(tmp_path, xlsx)
    with zipfile.ZipFile(r.outputs[0]) as z:
        assert "xl/comments1.xml" not in z.namelist()
        assert b"comments1" not in z.read("xl/worksheets/_rels/sheet1.xml.rels")
    assert not leftovers(tmp_path, r.outputs[0]) & (BAD | {"comments"})
    pptx = tmp_path / "in.pptx"
    with zipfile.ZipFile(pptx, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("docProps/core.xml", CORE)
        z.writestr("ppt/comments/comment1.xml", "<p:cmLst/>")
        z.writestr("ppt/commentAuthors.xml", "<p:cmAuthorLst/>")
    r = run(tmp_path / "p", pptx)
    with zipfile.ZipFile(r.outputs[0]) as z:
        assert z.namelist() == ["docProps/core.xml"]


def test_office_with_doctype_is_corrupt(tmp_path: Path) -> None:
    bad = tmp_path / "bad.docx"
    with zipfile.ZipFile(bad, "w") as z:
        z.writestr("docProps/core.xml", '<!DOCTYPE x [<!ENTITY a "b">]><x/>')
    with pytest.raises(AdapterError) as e:
        run_in_process(ADAPTER, tmp_path / "job", [bad], {})
    assert e.value.code == "E_INPUT_CORRUPT"
