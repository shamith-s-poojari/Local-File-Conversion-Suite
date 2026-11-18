"""file-clean removes what an Office file, a PDF or a PNG/WebP picture says
about who made it and where (audit PRIV-2, PRIV-4, PRIV-6)."""

from __future__ import annotations

import io
import re
import zipfile
import zlib
from pathlib import Path
from typing import Any

import pikepdf
import pytest
from converter_conformance.harness import context, run_in_process
from converter_sdk import reveal
from converter_sdk.errors import AdapterError
from converter_sdk.strip import icc_description
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
R = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
OFFICE_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"


def run(tmp_path: Path, src: Path, params: dict[str, Any] | None = None):
    r = run_in_process(ADAPTER, tmp_path / "job", [src], params or {})
    assert not r.errors, r.errors
    return r


def found(tmp_path: Path, path: Path) -> list[tuple[str, str | None]]:
    (tmp_path / "scan").mkdir(exist_ok=True)
    ctx = context(tmp_path / "scan", [])[0]
    return [(f.kind, f.detail) for f in reveal.findings(ctx, path, include_active=False)]


def rels(*items: tuple[str, str, str]) -> str:
    body = "".join(
        f'<Relationship Id="{i}" Type="{OFFICE_REL}/{t}" Target="{target}"/>'
        for i, t, target in items
    )
    return f'<Relationships xmlns="{REL_NS}">{body}</Relationships>'


def package(path: Path, parts: dict[str, str | bytes]) -> Path:
    overrides = "".join(
        f'<Override PartName="/{n}" ContentType="x/y"/>'
        for n in parts
        if n != "[Content_Types].xml" and not n.endswith(".rels")
    )
    ct = (
        f'<Types xmlns="{CT_NS}"><Default Extension="rels" ContentType="x/rels"/>'
        f"{overrides}</Types>"
    )
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", ct)
        for name, data in parts.items():
            z.writestr(name, data)
    return path


def read_all(path: Path) -> dict[str, bytes]:
    with zipfile.ZipFile(path) as z:
        return {n: z.read(n) for n in z.namelist()}


# ------------------------------------------------------------ core properties


def test_free_text_core_properties_are_removed(tmp_path: Path) -> None:
    core = (
        '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/'
        'core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">'
        "<dc:title>Merger plan</dc:title><dc:subject>Secret subject</dc:subject>"
        "<cp:keywords>kw-one</cp:keywords><dc:description>Notes to self</dc:description>"
        "<cp:category>Legal</cp:category><cp:contentStatus>Draft</cp:contentStatus>"
        "<dc:language>en-US</dc:language></cp:coreProperties>"
    )
    src = package(
        tmp_path / "in.docx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "word/document.xml")),
            "docProps/core.xml": core,
            "word/document.xml": f"<w:document {W}><w:body/></w:document>",
        },
    )
    assert (
        "tags",
        "Properties: Title, Subject, Keywords, Description, Category, Contentstatus",
    ) in (found(tmp_path, src))
    out = run(tmp_path, src).outputs[0]
    blob = read_all(out)["docProps/core.xml"]
    for gone in (b"Merger", b"Secret", b"kw-one", b"Notes", b"Legal", b"Draft"):
        assert gone not in blob
    assert b"en-US" in blob
    assert not [f for f in found(tmp_path, out) if f[0] == "tags"]


# ------------------------------------------------------ workbook path and user


def test_workbook_absolute_path_and_sharing_user_are_removed(tmp_path: Path) -> None:
    workbook = (
        '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
        'xmlns:x15ac="http://schemas.microsoft.com/office/spreadsheetml/2010/11/ac">'
        '<fileSharing readOnlyRecommended="1" userName="Jane Roe" reservationPassword="CA1B"/>'
        '<mc:AlternateContent><mc:Choice Requires="x15">'
        '<x15ac:absPath url="C:\\Users\\jane\\Documents\\"/></mc:Choice></mc:AlternateContent>'
        '<sheets><sheet name="S" sheetId="1"/></sheets></workbook>'
    )
    src = package(
        tmp_path / "in.xlsx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "xl/workbook.xml")),
            "xl/workbook.xml": workbook,
        },
    )
    kinds = found(tmp_path, src)
    assert ("author", "Path of the folder the file was saved in") in kinds
    assert ("author", "User name in the file sharing settings") in kinds
    out = run(tmp_path, src).outputs[0]
    data = read_all(out)["xl/workbook.xml"]
    for gone in (b"jane", b"Jane", b"absPath", b"AlternateContent", b"CA1B"):
        assert gone not in data
    assert b'readOnlyRecommended="1"' in data and b'name="S"' in data
    assert not [f for f in found(tmp_path, out) if f[0] == "author"]


# ------------------------------------------------- Word: ids, variables, names


def test_word_revision_ids_variables_and_permission_names_are_removed(tmp_path: Path) -> None:
    document = (
        f"<w:document {W}><w:body>"
        '<w:p w:rsidR="00AA11BB" w:rsidRDefault="00CC22DD" w:rsidP="00EE33FF">'
        '<w:permStart w:id="1" w:ed="jane@example.com" w:edGrp="everyone"/>'
        '<w:r w:rsidRPr="00112233"><w:t>text says w:rsidR="00AA11BB" and w:ed="x"</w:t></w:r>'
        '<w:permEnd w:id="1"/></w:p>'
        '<w:sectPr w:rsidSect="00445566"/></w:body></w:document>'
    )
    settings = (
        f'<w:settings {W}><w:docVars><w:docVar w:name="Author" w:val="Jane"/></w:docVars>'
        '<w:rsids><w:rsidRoot w:val="00AA11BB"/><w:rsid w:val="00CC22DD"/></w:rsids>'
        '<w:zoom w:percent="100"/></w:settings>'
    )
    styles = f'<w:styles {W}><w:style w:styleId="a"><w:rsid w:val="00778899"/></w:style></w:styles>'
    src = package(
        tmp_path / "in.docx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "word/document.xml")),
            "word/document.xml": document,
            "word/settings.xml": settings,
            "word/styles.xml": styles,
        },
    )
    before = found(tmp_path, src)
    assert ("tags", "Editing session IDs") in before
    assert ("tags", "Document variables") in before
    assert ("author", "People named in permission ranges") in before
    out = run(tmp_path, src).outputs[0]
    parts = read_all(out)
    doc = parts["word/document.xml"]
    assert b"w:rsidR=" not in doc.replace(b'text says w:rsidR="00AA11BB"', b"")
    assert b'text says w:rsidR="00AA11BB" and w:ed="x"' in doc  # text is not markup
    assert b"jane@example.com" not in doc and b'w:edGrp="everyone"' in doc
    assert b"<w:permStart" in doc and b"<w:permEnd" in doc
    assert b"rsid" not in parts["word/settings.xml"].replace(b"zoom", b"")
    assert b"docVar" not in parts["word/settings.xml"] and b"zoom" in parts["word/settings.xml"]
    assert b"rsid" not in parts["word/styles.xml"]
    for root in (doc, parts["word/settings.xml"], parts["word/styles.xml"]):
        assert re.match(rb"<(?:\?xml|w:)", root)
    assert not [f for f in found(tmp_path, out) if f[0] in ("tags", "author")]


def test_people_part_goes_even_when_comments_are_kept(tmp_path: Path) -> None:
    people = f'<w:people {W}><w:person w:author="Jane Roe"/></w:people>'
    document = (
        f'<w:document {W}><w:body><w:p><w:ins w:id="1" w:author="Jane Roe" '
        'w:date="2024-01-01T00:00:00Z"><w:r><w:t>new</w:t></w:r></w:ins></w:p></w:body>'
        "</w:document>"
    )
    src = package(
        tmp_path / "in.docx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "word/document.xml")),
            "word/document.xml": document,
            "word/people.xml": people,
            "word/_rels/document.xml.rels": rels(("rId1", "people", "people.xml")),
        },
    )
    assert ("author", "List of the people who edited the file") in found(tmp_path, src)
    out = run(tmp_path, src, {"removeComments": False}).outputs[0]
    parts = read_all(out)
    assert "word/people.xml" not in parts
    assert b"people" not in parts["word/_rels/document.xml.rels"]
    assert b"people" not in parts["[Content_Types].xml"]
    assert b"Jane" not in b"".join(parts.values())


# ----------------------------------- parts that carry other documents' details


def test_custom_xml_printer_settings_and_embeddings_are_removed(tmp_path: Path) -> None:
    sheet = (
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        f'{R}><sheetData/><pageSetup paperSize="9" r:id="rId1"/></worksheet>'
    )
    drawing_holder = (
        f"<w:document {W} {R}><w:body><w:p><w:r><w:object>"
        '<o:OLEObject xmlns:o="urn:schemas-microsoft-com:office:office" r:id="rId7"/>'
        "</w:object><w:t>kept</w:t></w:r></w:p></w:body></w:document>"
    )
    src = package(
        tmp_path / "in.docx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "word/document.xml")),
            "word/document.xml": drawing_holder,
            "word/_rels/document.xml.rels": rels(
                ("rId5", "customXml", "../customXml/item1.xml"),
                ("rId6", "printerSettings", "printerSettings/printerSettings1.bin"),
                ("rId7", "oleObject", "embeddings/oleObject1.bin"),
            ),
            "customXml/item1.xml": "<root>jane's sources</root>",
            "customXml/itemProps1.xml": "<props/>",
            "customXml/_rels/item1.xml.rels": rels(("rId1", "customXmlProps", "itemProps1.xml")),
            "word/printerSettings/printerSettings1.bin": b"\\\\server\\Jane's printer",
            "word/embeddings/oleObject1.bin": b"embedded Jane Roe",
        },
    )
    before = found(tmp_path, src)
    assert ("tags", "Custom XML data") in before
    assert ("device", "Printer settings") in before
    assert ("embeddedFiles", "1 embedded file") in before
    out = run(tmp_path, src).outputs[0]
    parts = read_all(out)
    assert not [n for n in parts if n.startswith(("customXml", "word/embeddings"))]
    assert not [n for n in parts if "printerSettings" in n]
    blob = b"".join(parts.values())
    for gone in (b"customXml", b"printerSettings", b"embeddings", b"Jane", b"jane"):
        assert gone not in blob, gone
    # The object's reference to the embedding goes with it; the text stays.
    assert b"rId7" not in parts["word/document.xml"] and b"kept" in parts["word/document.xml"]
    assert not [f for f in found(tmp_path, out) if f[0] in ("tags", "device", "embeddedFiles")]

    # A sheet's pageSetup points at its printer settings by id.
    xlsx = package(
        tmp_path / "in.xlsx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "xl/workbook.xml")),
            "xl/workbook.xml": '<workbook xmlns="http://schemas.openxmlformats.org/'
            'spreadsheetml/2006/main"/>',
            "xl/worksheets/sheet1.xml": sheet,
            "xl/worksheets/_rels/sheet1.xml.rels": rels(
                ("rId1", "printerSettings", "../printerSettings/printerSettings1.bin")
            ),
            "xl/printerSettings/printerSettings1.bin": b"printer",
        },
    )
    out2 = run(tmp_path / "x", xlsx).outputs[0]
    sheet_out = read_all(out2)["xl/worksheets/sheet1.xml"]
    assert b"rId1" not in sheet_out and b'paperSize="9"' in sheet_out


def test_part_copy_checks_for_cancel_in_every_chunk(tmp_path: Path, monkeypatch) -> None:
    import sys

    from converter_conformance.harness import load_adapter_class

    module = sys.modules[load_adapter_class(ADAPTER).__module__]
    monkeypatch.setattr(module, "CHUNK", 16)
    src = package(
        tmp_path / "in.docx",
        {
            "_rels/.rels": rels(("r1", "officeDocument", "word/document.xml")),
            "word/document.xml": f"<w:document {W}/>",
            "word/media/blob.bin": bytes(1000),
        },
    )

    class Ctx:
        calls = 0

        def check_cancelled(self) -> None:
            self.calls += 1

    ctx = Ctx()
    module.clean_ooxml(ctx, src, tmp_path / "out.docx", "docx", {})
    assert ctx.calls > 60  # one per 16 bytes of the 1000-byte part, not one per part


# --------------------------------------------------------------------- PDF


def make_pdf(path: Path) -> Path:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    page = pdf.pages[0].obj
    page["/LastModified"] = pikepdf.String("D:20240501100000Z")
    note = pdf.make_indirect(
        pikepdf.Dictionary(
            Type=pikepdf.Name.Annot,
            Subtype=pikepdf.Name.Text,
            Rect=[0, 0, 10, 10],
            Contents=pikepdf.String("remark"),
            T=pikepdf.String("Jane Roe"),
            M=pikepdf.String("D:20240501100000Z"),
            NM=pikepdf.String("id-1"),
        )  # fmt: skip
    )
    link = pdf.make_indirect(
        pikepdf.Dictionary(
            Type=pikepdf.Name.Annot,
            Subtype=pikepdf.Name.Link,
            Rect=[0, 0, 10, 10],
            M=pikepdf.String("D:20240501100000Z"),
            NM=pikepdf.String("id-2"),
        )  # fmt: skip
    )
    field = pdf.make_indirect(
        pikepdf.Dictionary(
            Type=pikepdf.Name.Annot,
            Subtype=pikepdf.Name.Widget,
            Rect=[0, 0, 10, 10],
            FT=pikepdf.Name.Tx,
            T=pikepdf.String("email"),
            M=pikepdf.String("D:20240501100000Z"),
        )  # fmt: skip
    )
    page["/Annots"] = pikepdf.Array([note, link, field])
    pdf.save(path)
    return path


def test_pdf_page_dates_annotation_names_and_id(tmp_path: Path) -> None:
    src = make_pdf(tmp_path / "in.pdf")
    before = found(tmp_path, src)
    assert ("dates", "Modification dates of pages or notes") in before
    assert ("author", "Names on annotations") in before
    with pikepdf.open(src) as original:
        old_id = [bytes(x) for x in original.trailer.ID]

    out = run(tmp_path, src, {"removeComments": False}).outputs[0]
    with pikepdf.open(out) as pdf:
        page = pdf.pages[0].obj
        assert "/LastModified" not in page
        by_type = {str(a.Subtype): a for a in page.Annots}
        for annot in by_type.values():
            assert "/M" not in annot and "/NM" not in annot
        assert "/T" not in by_type["/Text"]
        assert str(by_type["/Widget"].T) == "email"  # a field's name is not a person's
        new_id = [bytes(x) for x in pdf.trailer.ID]
    assert new_id[0] != old_id[0] and new_id[1] != old_id[1]
    assert not [f for f in found(tmp_path, out) if f[0] in ("dates", "author")]

    again = run_in_process(ADAPTER, tmp_path / "job2", [src], {"removeComments": False}).outputs[0]
    with pikepdf.open(again) as pdf:
        assert [bytes(x) for x in pdf.trailer.ID] == new_id  # repeat runs match


def test_pdf_links_lose_their_dates_when_comments_are_removed(tmp_path: Path) -> None:
    out = run(tmp_path, make_pdf(tmp_path / "in.pdf")).outputs[0]
    with pikepdf.open(out) as pdf:
        kept = {str(a.Subtype): a for a in pdf.pages[0].Annots}
        assert set(kept) == {"/Link", "/Widget"}
        assert all("/M" not in a and "/NM" not in a for a in kept.values())


# -------------------------------------------------------------- PNG and WebP


def chunk(kind: bytes, body: bytes) -> bytes:
    return len(body).to_bytes(4, "big") + kind + body + zlib.crc32(kind + body).to_bytes(4, "big")


def chunks_of(data: bytes) -> list[bytes]:
    pos, kinds = 8, []
    while pos < len(data):
        size = int.from_bytes(data[pos : pos + 4], "big")
        kinds.append(data[pos + 4 : pos + 8])
        pos += 12 + size
    return kinds


def make_icc(desc: str) -> bytes:
    ascii_ = desc.encode() + b"\x00"
    tag = b"desc" + bytes(4) + len(ascii_).to_bytes(4, "big") + ascii_ + bytes(4 + 4 + 2 + 1 + 67)
    size = 128 + 4 + 12 + len(tag)
    header = bytearray(128)
    header[0:4] = size.to_bytes(4, "big")
    header[8:12] = b"\x02\x40\x00\x00"
    header[12:24] = b"mntrRGB XYZ "
    header[36:40] = b"acsp"
    header[84:100] = b"\x11" * 16  # profile ID
    table = (
        (1).to_bytes(4, "big") + b"desc" + (144).to_bytes(4, "big") + len(tag).to_bytes(4, "big")
    )
    return bytes(header) + table + tag


def make_png(path: Path, extra: list[bytes]) -> Path:
    buf = io.BytesIO()
    Image.new("RGB", (8, 8), (10, 200, 30)).save(buf, "PNG")
    data = buf.getvalue()
    path.write_bytes(data[:33] + b"".join(extra) + data[33:])  # after IHDR
    return path


def test_png_keeps_only_listed_chunks_and_pixels(tmp_path: Path) -> None:
    src = make_png(
        tmp_path / "in.png",
        [
            chunk(b"tEXt", b"Author\x00Jane"), chunk(b"zTXt", b"k\x00\x00" + zlib.compress(b"v")),
            chunk(b"iTXt", b"k\x00\x00\x00\x00\x00Jane"), chunk(b"eXIf", b"MM\x00*"),
            chunk(b"tIME", bytes(7)), chunk(b"prVt", b"camera private data"),
            chunk(b"dSIG", b"signature"), chunk(b"pHYs", bytes(9)), chunk(b"gAMA", bytes(4)),
        ],
    )  # fmt: skip
    assert ("tags", "Private data chunks") in found(tmp_path, src)
    out = run(tmp_path, src).outputs[0]
    kinds = chunks_of(out.read_bytes())
    assert kinds[0] == b"IHDR" and kinds[-1] == b"IEND" and b"IDAT" in kinds
    assert set(kinds) == {b"IHDR", b"pHYs", b"gAMA", b"IDAT", b"IEND"}
    assert b"camera private" not in out.read_bytes()
    with Image.open(out) as new, Image.open(src) as old:
        assert new.tobytes() == old.tobytes()
    assert not [f for f in found(tmp_path, out) if f[0] in ("tags", "dates")]


def test_png_with_an_unknown_critical_chunk_is_refused(tmp_path: Path) -> None:
    src = make_png(tmp_path / "in.png", [chunk(b"ABCD", b"x")])
    with pytest.raises(AdapterError) as e:
        from converter_sdk.strip import clean_png_bytes

        clean_png_bytes(src.read_bytes())
    assert e.value.code == "E_INPUT_CORRUPT"


def test_png_icc_profile_keeps_its_colours_but_not_its_name(tmp_path: Path) -> None:
    profile = make_icc("Jane Roe's Laptop Display")
    body = b"Jane's Laptop\x00\x00" + zlib.compress(profile)
    out = run(tmp_path, make_png(tmp_path / "in.png", [chunk(b"iCCP", body)])).outputs[0]
    data = out.read_bytes()
    pos = data.index(b"iCCP")
    size = int.from_bytes(data[pos - 4 : pos], "big")
    new_body = data[pos + 4 : pos + 4 + size]
    assert new_body.startswith(b"ICC profile\x00\x00")
    new = zlib.decompress(new_body[13:])
    assert (
        len(new) == len(profile) and new[:84] == profile[:84] and new[100:128] == profile[100:128]
    )
    assert new[84:100] == bytes(16)  # the profile ID is a hash that names this exact profile
    assert icc_description(new) == "ICC profile"
    assert b"Jane" not in data
    assert (
        zlib.crc32(data[pos : pos + 4 + size]).to_bytes(4, "big")
        == data[pos + 4 + size : pos + 8 + size]
    )


def riff(chunks: list[tuple[bytes, bytes]]) -> bytes:
    body = b"WEBP" + b"".join(
        k + len(v).to_bytes(4, "little") + v + (b"\x00" if len(v) & 1 else b"") for k, v in chunks
    )
    return b"RIFF" + len(body).to_bytes(4, "little") + body


def test_webp_keeps_only_listed_chunks_and_a_neutral_profile(tmp_path: Path) -> None:
    buf = io.BytesIO()
    Image.new("RGB", (8, 8), (1, 2, 3)).save(buf, "WEBP", lossless=True)
    pic = riff([])
    inner = buf.getvalue()
    pos, sub = 12, []
    while pos < len(inner):
        size = int.from_bytes(inner[pos + 4 : pos + 8], "little")
        sub.append((inner[pos : pos + 4], inner[pos + 8 : pos + 8 + size]))
        pos += 8 + size + (size & 1)
    assert pic and sub[0][0] in (b"VP8L", b"VP8 ")
    vp8x = bytes([0x20 | 0x08 | 0x04]) + bytes(3) + (7).to_bytes(3, "little") * 2
    profile = make_icc("Jane Roe's Laptop Display")
    src = tmp_path / "in.webp"
    src.write_bytes(
        riff([(b"VP8X", vp8x), (b"ICCP", profile), *sub, (b"EXIF", b"Jane"), (b"XMP ", b"Jane"),
              (b"JUNK", b"camera private data"), (b"SERI", b"serial 123")])
    )  # fmt: skip
    out = run(tmp_path, src).outputs[0]
    data = out.read_bytes()
    kinds = []
    pos = 12
    while pos < len(data):
        size = int.from_bytes(data[pos + 4 : pos + 8], "little")
        kinds.append(data[pos : pos + 4])
        pos += 8 + size + (size & 1)
    assert kinds == [b"VP8X", b"ICCP", sub[0][0]]
    assert b"Jane" not in data and b"camera" not in data and b"serial" not in data
    assert data[20] & 0x0C == 0 and data[20] & 0x20  # EXIF/XMP flags off, ICC flag stays
    assert icc_description(data[38 : 38 + len(profile)]) == "ICC profile"
