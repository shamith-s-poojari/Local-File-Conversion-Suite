"""What reveal reports about Office parts, PDF notes and PNG chunks that the
cleaning tool removes (audit PRIV-2, PRIV-4, PRIV-6)."""

from __future__ import annotations

import io
import zipfile
import zlib
from pathlib import Path

import pikepdf
from converter_sdk import reveal
from converter_sdk.protocol import Finding
from PIL import Image

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'


def pairs(found: list[Finding]) -> set[tuple[str, str | None]]:
    return {(f.kind, f.detail) for f in found}


def docx(path: Path, parts: dict[str, bytes | str]) -> Path:
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("[Content_Types].xml", "<Types/>")
        for n, d in parts.items():
            z.writestr(n, d)
    return path


def test_office_parts_are_reported_once_by_the_right_reader(tmp_path: Path) -> None:
    src = docx(
        tmp_path / "a.docx",
        {
            "word/document.xml": f'<w:document {W}><w:p w:rsidR="00AA11BB"/></w:document>',
            "customXml/item1.xml": "<x/>",
            "word/embeddings/o.bin": b"x",
            "word/printerSettings/p.bin": b"x",
        },
    )
    mine = pairs(reveal.ooxml_findings(src, external=True))
    assert {
        ("tags", "Editing session IDs"),
        ("tags", "Custom XML data"),
        ("embeddedFiles", "1 embedded file"),
        ("device", "Printer settings"),
    } <= mine
    # With the active-content reader in charge, embedded files and custom XML are its.
    theirs = pairs(reveal.ooxml_findings(src, external=False))
    assert ("device", "Printer settings") in theirs
    assert (
        not {k for k, _ in theirs} & {"embeddedFiles"} and ("tags", "Custom XML data") not in theirs
    )


def test_a_clean_word_part_has_no_identity_findings(tmp_path: Path) -> None:
    src = docx(tmp_path / "b.docx", {"word/document.xml": f"<w:document {W}><w:p/></w:document>"})
    assert not [f for f in reveal.ooxml_findings(src) if f.kind in ("tags", "author")]


def test_pdf_dates_and_note_authors_are_reported(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.pages[0].obj["/LastModified"] = pikepdf.String("D:20240501")
    note = pdf.make_indirect(
        pikepdf.Dictionary(Subtype=pikepdf.Name.Text, T=pikepdf.String("Jane"), Rect=[0, 0, 1, 1])
    )
    pdf.pages[0].obj["/Annots"] = pikepdf.Array([note])
    found = pairs(reveal.pdf_findings(pdf))
    assert ("dates", "Modification dates of pages or notes") in found
    assert ("author", "Names on annotations") in found


def test_png_private_chunks_are_reported() -> None:
    buf = io.BytesIO()
    Image.new("RGB", (2, 2)).save(buf, "PNG")
    data = buf.getvalue()
    private = b"prVt"
    body = b"serial"
    extra = (
        len(body).to_bytes(4, "big") + private + body
        + zlib.crc32(private + body).to_bytes(4, "big")
    )  # fmt: skip
    assert ("tags", "Private data chunks") in pairs(
        reveal.image_bytes_findings(data[:33] + extra + data[33:])
    )
    assert ("tags", "Private data chunks") not in pairs(reveal.image_bytes_findings(data))
