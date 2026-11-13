"""Acceptance tests for markup-convert (ADR 0003)."""

from __future__ import annotations

import base64
import io
import zipfile
from pathlib import Path

import pytest
import yaml
from converter_conformance import files, validate
from converter_conformance.harness import load_adapter_class, run_in_process
from converter_sdk import AdapterError

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)


@pytest.fixture(scope="module")
def sources(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    d = tmp_path_factory.mktemp("markup")
    return {f: files.build(f"doc-{f}", d) for f in ["md", "txt", "html", "epub"]}


def convert(tmp_path: Path, src: Path, to: str, **params):
    return run_in_process(
        ADAPTER, tmp_path / "job", [src], {"to": to, "fileTitles": ["report"], **params}
    )


def pdf_text(path: Path) -> str:
    import pymupdf  # noqa: PLC0415

    with pymupdf.open(path) as doc:
        return " ".join(page.get_text() for page in doc)


def make_epub(
    path: Path, chapters: list[str], *, image: bytes | None = None, encryption: str | None = None
) -> Path:
    manifest = "".join(
        f'<item id="c{i}" href="text/c{i}.xhtml" media-type="application/xhtml+xml"/>'
        for i in range(len(chapters))
    )
    if image is not None:
        manifest += '<item id="img" href="images/pic.png" media-type="image/png"/>'
    spine = "".join(f'<itemref idref="c{i}"/>' for i in range(len(chapters)))
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip")
        z.writestr(
            "META-INF/container.xml",
            '<?xml version="1.0"?><container version="1.0" '
            'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
            '<rootfile full-path="OEBPS/book.opf" '
            'media-type="application/oebps-package+xml"/></rootfiles></container>',
        )
        z.writestr(
            "OEBPS/book.opf",
            '<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" '
            'version="3.0" unique-identifier="i">'
            '<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">'
            '<dc:identifier id="i">x</dc:identifier>'
            "<dc:title>T</dc:title><dc:language>en</dc:language></metadata>"
            f"<manifest>{manifest}</manifest><spine>{spine}</spine></package>",
        )
        for i, body in enumerate(chapters):
            z.writestr(
                f"OEBPS/text/c{i}.xhtml",
                '<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml">'
                f"<head><title>c{i}</title><script>alert(1)</script></head><body>{body}</body></html>",
            )
        if image is not None:
            z.writestr("OEBPS/images/pic.png", image)
        if encryption is not None:
            z.writestr(
                "META-INF/encryption.xml",
                '<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" '
                'xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData>'
                f'<enc:EncryptionMethod Algorithm="{encryption}"/>'
                '<enc:CipherData><enc:CipherReference URI="OEBPS/text/c0.xhtml"/></enc:CipherData>'
                "</enc:EncryptedData></encryption>",
            )
    return path


def test_manifest_edges_match_the_adapter_table() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
    assert declared == CLS.EDGES


@pytest.mark.parametrize(("src", "to"), sorted(CLS.EDGES))
def test_every_pair(tmp_path: Path, sources, src: str, to: str) -> None:
    r = convert(tmp_path, sources[src], to)
    assert not r.errors, r.errors
    assert r.names == [f"report.{to}"]
    validate.check(r.outputs[0], to)
    text = pdf_text(r.outputs[0]) if to == "pdf" else r.outputs[0].read_text("utf-8")
    keyword = "Lisbon" if src != "md" else "Revenue grew"
    assert keyword in text


def test_markdown_to_html_is_safe_and_structured(tmp_path: Path) -> None:
    src = tmp_path / "notes.md"
    src.write_text(
        "# Title\n\n<script>alert(1)</script>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n~~old~~\n",
        "utf-8",
    )
    html = convert(tmp_path, src, "html").outputs[0].read_text("utf-8")
    assert "<h1>Title</h1>" in html and "<td>1</td>" in html and "<s>old</s>" in html
    assert "<script>" not in html and "&lt;script&gt;" in html


def test_markdown_to_pdf_keeps_every_script(tmp_path: Path) -> None:
    src = tmp_path / "world.md"
    src.write_text("# Hello\n\nGreek αβγ, Cyrillic Жж, CJK 漢字, Devanagari नमस्ते.\n", "utf-8")
    text = pdf_text(convert(tmp_path, src, "pdf").outputs[0])
    for word in ["Hello", "αβγ", "Жж", "漢字"]:
        assert word in text


def test_page_size_is_used(tmp_path: Path, sources) -> None:
    import pymupdf  # noqa: PLC0415

    for size, width in [("a4", 595), ("letter", 612)]:
        with pymupdf.open(
            convert(tmp_path / size, sources["md"], "pdf", pageSize=size).outputs[0]
        ) as doc:
            assert round(doc[0].rect.width) == width


def test_pdf_output_repeats_exactly(tmp_path: Path, sources) -> None:
    a = convert(tmp_path / "a", sources["md"], "pdf").outputs[0].read_bytes()
    b = convert(tmp_path / "b", sources["md"], "pdf").outputs[0].read_bytes()
    assert a == b


def test_html_to_pdf_fetches_nothing_and_runs_nothing(tmp_path: Path) -> None:
    pixel = base64.b64encode(_png()).decode()
    src = tmp_path / "page.html"
    src.write_text(
        "<html><head><link rel=stylesheet href='https://example.com/x.css'>"
        "<script>document.write('INJECTED')</script></head><body><h1>Title</h1>"
        "<img src='https://example.com/track.png'><img src='file:///etc/passwd'>"
        f"<img src='data:image/png;base64,{pixel}'><iframe src='https://example.com'></iframe>"
        "<p>Body text</p></body></html>",
        "utf-8",
    )
    r = convert(tmp_path, src, "pdf")
    import pymupdf  # noqa: PLC0415

    with pymupdf.open(r.outputs[0]) as doc:
        text = doc[0].get_text()
        assert "Title" in text and "Body text" in text and "INJECTED" not in text
        assert len(doc[0].get_images()) == 1  # only the embedded one
    assert "IMAGES_DROPPED" in r.warnings


def test_html_to_markdown_drops_scripts_and_images(tmp_path: Path) -> None:
    src = tmp_path / "page.html"
    src.write_text(
        "<html><body><script>alert(1)</script><h2>Title</h2><p>Hello <b>there</b> "
        "<img src='https://example.com/x.png'></p></body></html>",
        "utf-8",
    )
    r = convert(tmp_path, src, "md")
    assert r.outputs[0].read_text("utf-8") == "## Title\n\nHello **there**\n"
    assert "IMAGES_DROPPED" in r.warnings


def test_html_to_text_skips_scripts(tmp_path: Path) -> None:
    src = tmp_path / "page.html"
    src.write_text(
        "<html><body><script>var x=1</script><h2>Hi</h2><p>There</p></body></html>", "utf-8"
    )
    assert convert(tmp_path, src, "txt").outputs[0].read_text("utf-8") == "Hi\n\nThere\n"


def test_legacy_encodings_are_read(tmp_path: Path) -> None:
    src = tmp_path / "old.txt"
    src.write_bytes("Café crème".encode("cp1252"))
    assert "Café crème" in convert(tmp_path, src, "md").outputs[0].read_text("utf-8")
    src = tmp_path / "wide.txt"
    src.write_bytes("﻿Wide text".encode("utf-16-le"))
    assert "Wide text" in convert(tmp_path / "b", src, "md").outputs[0].read_text("utf-8")


def test_epub_chapters_in_reading_order_with_images(tmp_path: Path) -> None:
    book = make_epub(
        tmp_path / "book.epub",
        ["<h1>One</h1><p>First <a href='c1.xhtml#end'>link</a></p><img src='../images/pic.png'/>",
         "<h1 id='end'>Two</h1><p>Second</p>"],
        image=_png(),
    )  # fmt: skip
    html = convert(tmp_path / "h", book, "html").outputs[0].read_text("utf-8")
    assert html.index("One") < html.index("Two")
    assert "data:image/png;base64," in html and "<script" not in html
    assert 'href="#end"' in html
    md = convert(tmp_path / "m", book, "md")
    assert md.outputs[0].read_text("utf-8").startswith("# One\n\nFirst [link](#end)")
    assert "IMAGES_DROPPED" in md.warnings
    assert (
        convert(tmp_path / "t", book, "txt").outputs[0].read_text("utf-8")
        == "One\n\nFirst link\n\nTwo\n\nSecond\n"
    )
    pdf = convert(tmp_path / "p", book, "pdf")
    assert "Second" in pdf_text(pdf.outputs[0])


def test_drm_books_are_refused(tmp_path: Path) -> None:
    book = make_epub(
        tmp_path / "drm.epub",
        ["<p>x</p>"],
        encryption="http://www.w3.org/2001/04/xmlenc#aes128-cbc",
    )
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, book, "pdf")
    assert e.value.code == "E_INPUT_CORRUPT"
    fonts = make_epub(
        tmp_path / "fonts.epub", ["<p>x</p>"], encryption="http://www.idpf.org/2008/embedding"
    )
    assert convert(tmp_path / "ok", fonts, "txt").outputs


def test_xml_entities_are_not_expanded(tmp_path: Path) -> None:
    book = tmp_path / "xxe.epub"
    make_epub(book, ["<p>x</p>"])
    with zipfile.ZipFile(book, "a") as z:
        z.writestr(
            "META-INF/container.xml",
            '<?xml version="1.0"?><!DOCTYPE c [<!ENTITY x SYSTEM "file:///etc/passwd">]>'
            '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
            '<rootfile full-path="&x;"/></rootfiles></container>',
        )
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, book, "txt")
    assert e.value.code == "E_INPUT_CORRUPT"


def test_empty_text_is_refused(tmp_path: Path) -> None:
    src = tmp_path / "blank.html"
    src.write_text("<html><body><script>x</script></body></html>", "utf-8")
    with pytest.raises(AdapterError) as e:
        convert(tmp_path, src, "txt")
    assert e.value.code == "E_NO_TEXT"


def _png() -> bytes:
    from PIL import Image  # noqa: PLC0415

    buf = io.BytesIO()
    Image.new("RGB", (8, 8), (200, 40, 40)).save(buf, "PNG")
    return buf.getvalue()
