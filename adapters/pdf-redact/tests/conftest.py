"""Adversarial fixtures for pdf-redact (spec 13.8 acceptance table).

Every fixture hides known secret strings. The tests fail if any secret can
be recovered from the output by any engine, or in raw bytes.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pikepdf
import pymupdf
import pytest

SECRET = "Zorblax7731"
SECRET2 = "Quintessa4410"
KEEP = "Visible-Keeper"
"""Text that must survive, next to every secret, so a misplaced box is caught."""


def _font_page(doc: pymupdf.Document, **kw) -> pymupdf.Page:
    return doc.new_page(width=kw.get("width", 612), height=kw.get("height", 792))


def plain(path: Path) -> Path:
    doc = pymupdf.open()
    page = _font_page(doc)
    page.insert_text((72, 100), f"Account {SECRET} belongs to the client.", fontsize=12)
    page.insert_text((72, 140), f"{KEEP} stays readable.", fontsize=12)
    page.insert_text((72, 180), f"Second mention: {SECRET} again.", fontsize=12)
    doc.save(path)
    return path


def rotated(path: Path, rotate: int, crop: tuple[float, float] | None = None) -> Path:
    doc = pymupdf.open()
    page = _font_page(doc)
    page.insert_text((100, 300), SECRET, fontsize=14)
    page.insert_text((100, 500), KEEP, fontsize=14)
    if crop is not None:
        page.set_cropbox(pymupdf.Rect(crop[0], crop[1], 612, 792))
    page.set_rotation(rotate)
    doc.save(path)
    return path


def split_ops_and_lines(path: Path) -> Path:
    """The secret split across content operators, and a second one hyphenated
    across a line break."""
    pdf = pikepdf.new()
    font = pdf.make_indirect(
        pikepdf.Dictionary(
            Type=pikepdf.Name.Font, Subtype=pikepdf.Name.Type1, BaseFont=pikepdf.Name.Helvetica
        )
    )
    half = len(SECRET2) // 2
    content = (
        f"BT /F1 12 Tf 72 700 Td (Ref: {SECRET[:4]}) Tj ({SECRET[4:]}) Tj ET\n"
        f"BT /F1 12 Tf 72 660 Td (Contact {SECRET2[:half]}-) Tj ET\n"
        f"BT /F1 12 Tf 72 645 Td ({SECRET2[half:]} today.) Tj ET\n"
        f"BT /F1 12 Tf 72 600 Td ({KEEP}) Tj ET\n"
    ).encode("latin-1")
    page = pikepdf.Dictionary(
        Type=pikepdf.Name.Page,
        MediaBox=[0, 0, 612, 792],
        Resources=pikepdf.Dictionary(Font=pikepdf.Dictionary(F1=font)),
        Contents=pdf.make_stream(content),
    )
    pdf.pages.append(pikepdf.Page(pdf.make_indirect(page)))
    pdf.save(path)
    return path


def hidden_text(path: Path) -> Path:
    """Invisible text (render mode 3) and text under an image."""
    doc = pymupdf.open()
    page = _font_page(doc)
    page.insert_text((72, 100), SECRET, fontsize=12, render_mode=3)
    page.insert_text((72, 300), SECRET2, fontsize=12)
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 50, 20), False)
    pix.set_rect(pix.irect, (40, 90, 200))
    page.insert_image(pymupdf.Rect(60, 280, 300, 310), pixmap=pix)
    page.insert_text((72, 500), KEEP, fontsize=12)
    doc.save(path)
    return path


def form_xobject(path: Path) -> Path:
    """Text inside a Form XObject (show_pdf_page wraps the source page in one)."""
    inner = pymupdf.open()
    p = inner.new_page(width=612, height=792)
    p.insert_text((72, 100), SECRET, fontsize=12)
    doc = pymupdf.open()
    page = _font_page(doc)
    page.show_pdf_page(page.rect, inner, 0)
    page.insert_text((72, 500), KEEP, fontsize=12)
    doc.save(path)
    return path


def image_under_box(path: Path, rotate: int = 0, crop: tuple[float, float] | None = None) -> Path:
    doc = pymupdf.open()
    page = _font_page(doc)
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 200, 200), False)
    pix.set_rect(pix.irect, (220, 30, 30))
    page.insert_image(pymupdf.Rect(100, 100, 300, 300), pixmap=pix)
    page.insert_text((72, 500), KEEP, fontsize=12)
    if crop is not None:
        page.set_cropbox(pymupdf.Rect(crop[0], crop[1], 612, 792))
    page.set_rotation(rotate)
    doc.save(path)
    return path


def document_level(path: Path) -> Path:
    """Secrets in metadata, XMP, attachments, annotations, form values,
    bookmarks, thumbnails and a hidden layer. Page text has only KEEP."""
    doc = pymupdf.open()
    page = _font_page(doc)
    page.insert_text((72, 100), KEEP, fontsize=12)
    page2 = _font_page(doc)
    page2.insert_text((72, 100), f"Chapter {SECRET}", fontsize=12)
    page = doc[0]  # adding a page invalidates earlier page handles
    doc.set_metadata({"title": f"Report {SECRET}", "author": SECRET2, "subject": SECRET})
    doc.set_xml_metadata(
        f'<?xpacket begin=""?><x:xmpmeta xmlns:x="adobe:ns:meta/">{SECRET}</x:xmpmeta>'
    )
    doc.embfile_add("notes.txt", f"attached {SECRET}".encode(), filename="notes.txt")
    page.add_text_annot((300, 300), f"comment {SECRET}")
    widget = pymupdf.Widget()
    widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
    widget.field_name = "account"
    widget.field_value = SECRET2
    widget.rect = pymupdf.Rect(72, 200, 300, 230)
    page.add_widget(widget)
    doc.set_toc([[1, f"Section {SECRET}", 2], [1, "Overview", 1]])
    ocg = doc.add_ocg("Hidden", on=False)
    page.insert_text((72, 400), f"layer {SECRET2}", fontsize=12, oc=ocg)
    doc.save(path)
    with pikepdf.open(path, allow_overwriting_input=True) as pdf:
        pdf.pages[0].obj.Thumb = pdf.make_stream(f"thumb {SECRET}".encode())
        pdf.save(path)
    return path


def incremental_history(path: Path) -> Path:
    """The secret was in an earlier revision; the visible text was replaced by
    an incremental save, so the old content is still in the file."""
    doc = pymupdf.open()
    page = _font_page(doc)
    page.insert_text((72, 100), f"Old {SECRET}", fontsize=12)
    doc.save(path)
    doc.close()
    doc = pymupdf.open(path)
    page = doc[0]
    page.add_redact_annot(page.rect)
    page.apply_redactions()
    page.insert_text((72, 100), KEEP, fontsize=12)
    doc.saveIncr()
    doc.close()
    return path


def scanned(path: Path) -> Path:
    """One page that is only an image: no text layer."""
    doc = pymupdf.open()
    page = _font_page(doc)
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 300, 400), False)
    pix.set_rect(pix.irect, (235, 235, 235))
    pix.set_rect(pymupdf.IRect(50, 50, 250, 80), (10, 10, 10))
    page.insert_image(page.rect, pixmap=pix)
    doc.save(path)
    return path


BUILDERS: dict[str, Callable[[Path], Path]] = {
    "plain": plain,
    "rot-0": lambda p: rotated(p, 0),
    "rot-90": lambda p: rotated(p, 90),
    "rot-180": lambda p: rotated(p, 180),
    "rot-270": lambda p: rotated(p, 270),
    "rot-90-crop": lambda p: rotated(p, 90, crop=(40, 60)),
    "crop": lambda p: rotated(p, 0, crop=(50, 80)),
    "split": split_ops_and_lines,
    "hidden": hidden_text,
    "xobject": form_xobject,
    "image": image_under_box,
    "image-rot-crop": lambda p: image_under_box(p, 90, crop=(40, 60)),
    "document": document_level,
    "incremental": incremental_history,
    "scanned": scanned,
}


@pytest.fixture
def fixture(tmp_path: Path) -> Callable[[str], Path]:
    def build(name: str) -> Path:
        path = tmp_path / "fixtures" / f"{name}.pdf"
        path.parent.mkdir(exist_ok=True)
        return BUILDERS[name](path)

    return build
