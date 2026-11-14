"""markup-convert: Markdown, plain text, HTML and EPUB, without LibreOffice
(ADR 0003).

PDFs are laid out by MuPDF's HTML engine (``Story``) on A4 or Letter pages,
with fallback fonts for non-Latin scripts. EPUBs become PDFs through MuPDF's
own EPUB layout. Nothing is ever fetched: images a document points at
elsewhere are left out (``IMAGES_DROPPED``), and images inside an EPUB are
embedded in the HTML.
"""

from __future__ import annotations

import base64
import io
import posixpath
import zipfile
from pathlib import Path
from typing import Any, ClassVar
from urllib.parse import unquote, urlsplit

from converter_sdk import (
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import ConversionAdapter, Tier, pdf_bytes_to_output, source_format
from converter_sdk.markup import (
    PAGE_CSS,
    check_zip,
    html_document,
    html_to_markdown,
    html_to_text,
    markdown_to_html,
    markdown_to_text,
    parse_html,
    read_text,
    text_to_html,
    text_to_markdown,
)

MB = 1024 * 1024
EDGES: dict[tuple[str, str], Tier] = {
    ("md", "html"): "H", ("md", "pdf"): "H", ("md", "txt"): "L",
    ("txt", "html"): "H", ("txt", "md"): "H", ("txt", "pdf"): "H",
    ("html", "md"): "R", ("html", "txt"): "L", ("html", "pdf"): "H",
    ("epub", "pdf"): "H", ("epub", "html"): "R", ("epub", "md"): "R", ("epub", "txt"): "L",
}  # fmt: skip
MAX_TEXT_BYTES = 64 * MB
MAX_PAGES = 5000
MARGIN = 54
"""Three quarters of an inch."""
EPUB_CHUNK = 10
"""EPUB pages laid out at a time, so cancelling is quick."""
EPUB_MAX_MEMBER = 200 * MB
EPUB_MAX_TOTAL = 1024 * MB
EPUB_MAX_IMAGES = 64 * MB
"""Images embedded into one HTML file, in total; more are left out."""
FONT_OBFUSCATION = {"http://www.idpf.org/2008/embedding", "http://ns.adobe.com/pdf/enc#RC"}
"""Encryption an EPUB may use for its fonts only; anything else is DRM."""


class MarkupConvertAdapter(ConversionAdapter):
    adapter_id: ClassVar[str] = "markup-convert"
    EDGES = EDGES

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        sizes = []
        for p in ctx.inputs:
            size = p.stat().st_size
            if size == 0:
                raise AdapterError("E_INPUT_CORRUPT")
            if source_format(p) == "epub":
                Epub(p).close()  # damaged, oversized or DRM-locked books stop here
            elif size > MAX_TEXT_BYTES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "bytes", "max": MAX_TEXT_BYTES})
            sizes.append(size)
        return ValidationReport(metadata={"bytes": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        largest = max(metadata.get("bytes") or [0])
        return ResourceEstimate(
            ram_bytes=160 * MB + largest * 30, disk_bytes=largest * 8 + 20 * MB, cost_units=2
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = self.target(params)
        self.use_tiers(ctx, to)
        self._warned: set[str] = set()
        outputs = []
        total = len(ctx.inputs)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("convert", i + 1, total)
            outputs.append(self._one(ctx, path, source_format(path), to, params, i))
        return outputs

    # ---------------------------------------------------------------- steps

    def _one(
        self, ctx: JobContext, path: Path, src: str, to: str, params: dict[str, Any], i: int
    ) -> OutputFile:
        title = self.title(params, i)
        out = ctx.output_path(f"{title}.{to}")
        size = str(params.get("pageSize") or "a4")
        if src == "epub":
            book = Epub(path)
            try:
                if to == "pdf":
                    book.close()
                    pages = self._epub_pdf(ctx, path, out, size)
                    return OutputFile(out, pages=pages)
                body, dropped = book.body(embed_images=to == "html")
            finally:
                book.close()
            if dropped:
                self._warn(ctx, "IMAGES_DROPPED")
            if to == "html":
                out.write_text(html_document(body, title), "utf-8", newline="\n")
            elif to == "md":
                md, _ = html_to_markdown(f"<html><body>{body}</body></html>")
                out.write_text(md, "utf-8", newline="\n")
            else:
                out.write_text(
                    html_to_text(f"<html><body>{body}</body></html>"), "utf-8", newline="\n"
                )
            return OutputFile(out)

        text = read_text(path)
        if to == "pdf":
            html = {
                "md": lambda: markdown_to_html(text, title),
                "txt": lambda: text_to_html(text, title),
            }.get(src, lambda: text)()
            clean, dropped = _for_layout(html)
            if dropped:
                self._warn(ctx, "IMAGES_DROPPED")
            pages = _layout_pdf(ctx, clean, out, size, len(text))
            return OutputFile(out, pages=pages)
        if (src, to) == ("md", "html"):
            result = markdown_to_html(text, title)
        elif (src, to) == ("md", "txt"):
            result = markdown_to_text(text)
        elif (src, to) == ("txt", "html"):
            result = text_to_html(text, title)
        elif (src, to) == ("txt", "md"):
            result = text_to_markdown(text)
        elif (src, to) == ("html", "md"):
            result, dropped = html_to_markdown(text)
            if dropped:
                self._warn(ctx, "IMAGES_DROPPED")
        else:  # html → txt
            result = html_to_text(text)
        if not result.strip():
            raise AdapterError("E_NO_TEXT")
        out.write_text(result, "utf-8", newline="\n")
        return OutputFile(out)

    def _epub_pdf(self, ctx: JobContext, path: Path, out: Path, size: str) -> int:
        import pymupdf  # noqa: PLC0415

        try:
            book = pymupdf.open(path, filetype="epub")
            box = pymupdf.paper_rect(size)
            book.layout(rect=box, fontsize=11)
            count = book.page_count
        except Exception:  # noqa: BLE001 - MuPDF raises plain RuntimeErrors
            raise AdapterError("E_INPUT_CORRUPT") from None
        try:
            if count == 0:
                raise AdapterError("E_INPUT_CORRUPT")
            if count > MAX_PAGES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "pages", "max": MAX_PAGES})
            pdf = pymupdf.open()
            for start in range(0, count, EPUB_CHUNK):
                ctx.check_cancelled()
                end = min(count, start + EPUB_CHUNK) - 1
                chunk = pymupdf.open("pdf", book.convert_to_pdf(from_page=start, to_page=end))
                pdf.insert_pdf(chunk)
                chunk.close()
                ctx.progress("convert", end + 1, count)
            data = pdf.tobytes()
            pdf.close()
        finally:
            book.close()
        return pdf_bytes_to_output(ctx, data, out)

    def _warn(self, ctx: JobContext, code: str) -> None:
        if code not in self._warned:
            self._warned.add(code)
            ctx.warn(code)


# ---------------------------------------------------------------- layout


def _parser() -> Any:
    from lxml import html as lxml_html  # noqa: PLC0415

    return lxml_html.HTMLParser(no_network=True, remove_comments=True, remove_pis=True)


def _for_layout(source: str) -> tuple[str, bool]:
    """HTML that's safe to lay out: no scripts, frames, plugins or links to
    other files, and only images carried inside the document itself."""
    from lxml import html as lxml_html  # noqa: PLC0415

    tree = parse_html(source)
    for bad in tree.xpath(
        "//script|//iframe|//frame|//object|//embed|//link|//noscript|//template|//base"
    ):
        bad.drop_tree()
    dropped = False
    for img in tree.xpath("//img|//image|//video|//audio|//source"):
        src = (
            img.get("src") or img.get("href") or img.get("{http://www.w3.org/1999/xlink}href") or ""
        ).strip()
        if not src.lower().startswith("data:image/"):
            img.drop_tree()
            dropped = True
    return lxml_html.tostring(tree, encoding="unicode", doctype="<!DOCTYPE html>"), dropped


def _layout_pdf(ctx: JobContext, html: str, out: Path, size: str, chars: int) -> int:
    import pymupdf  # noqa: PLC0415

    try:
        story = pymupdf.Story(html=html, user_css=PAGE_CSS)
    except Exception:  # noqa: BLE001
        raise AdapterError("E_INPUT_CORRUPT") from None
    buf = io.BytesIO()
    writer = pymupdf.DocumentWriter(buf)
    box = pymupdf.paper_rect(size)
    where = box + (MARGIN, MARGIN, -MARGIN, -MARGIN)
    guess = max(1, chars // 2500 + 1)
    pages, empty, more = 0, 0, True
    try:
        while more:
            ctx.check_cancelled()
            device = writer.begin_page(box)
            more, filled = story.place(where)
            story.draw(device)
            writer.end_page()
            pages += 1
            # Something that can never fit (a huge picture) would ask for
            # pages forever.
            empty = empty + 1 if pymupdf.Rect(filled).is_empty else 0
            if pages > MAX_PAGES or empty > 3:
                raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "pages", "max": MAX_PAGES})
            ctx.progress("convert", pages, max(guess, pages + (1 if more else 0)))
    finally:
        writer.close()
    return pdf_bytes_to_output(ctx, buf.getvalue(), out)


# ------------------------------------------------------------------- EPUB


class Epub:
    """An EPUB's chapters in reading order. Sizes are checked before anything
    is read; XML is parsed without entities or network access."""

    def __init__(self, path: Path) -> None:
        try:
            self.zip = zipfile.ZipFile(path)
        except (zipfile.BadZipFile, OSError):
            raise AdapterError("E_INPUT_CORRUPT") from None
        try:
            infos = check_zip(self.zip, member_max=EPUB_MAX_MEMBER, total_max=EPUB_MAX_TOTAL)
            self.names = {i.filename for i in infos}
            self._refuse_drm()
            container = self._xml("META-INF/container.xml")
            rootfile = container.xpath("//*[local-name()='rootfile']/@full-path")
            if not rootfile:
                raise AdapterError("E_INPUT_CORRUPT")
            self.opf_path = str(rootfile[0])
            opf = self._xml(self.opf_path)
            base = posixpath.dirname(self.opf_path)
            self.media: dict[str, str] = {}
            items: dict[str, str] = {}
            for item in opf.xpath("//*[local-name()='manifest']/*[local-name()='item']"):
                href = posixpath.normpath(posixpath.join(base, unquote(item.get("href", ""))))
                items[item.get("id", "")] = href
                self.media[href] = item.get("media-type", "")
            self.spine = [
                items[ref]
                for ref in opf.xpath("//*[local-name()='spine']/*[local-name()='itemref']/@idref")
                if ref in items and items[ref] in self.names
            ]
            if not self.spine:
                raise AdapterError("E_INPUT_CORRUPT")
        except AdapterError:
            self.zip.close()
            raise
        except Exception:  # noqa: BLE001 - malformed XML, bad zip members
            self.zip.close()
            raise AdapterError("E_INPUT_CORRUPT") from None

    def close(self) -> None:
        self.zip.close()

    def _refuse_drm(self) -> None:
        if "META-INF/encryption.xml" not in self.names:
            return
        enc = self._xml("META-INF/encryption.xml")
        methods = enc.xpath("//*[local-name()='EncryptionMethod']/@Algorithm")
        if any(m not in FONT_OBFUSCATION for m in methods):
            raise AdapterError("E_INPUT_CORRUPT", {"reason": "drm"})

    def _xml(self, name: str) -> Any:
        from lxml import etree  # noqa: PLC0415

        parser = etree.XMLParser(
            resolve_entities=False, no_network=True, load_dtd=False, huge_tree=False
        )
        return etree.fromstring(self.zip.read(name), parser=parser)

    def body(self, embed_images: bool) -> tuple[str, bool]:
        """Every chapter's body as one HTML fragment, and whether images were left out."""
        from lxml import html as lxml_html  # noqa: PLC0415

        parts = []
        dropped = False
        budget = EPUB_MAX_IMAGES
        for chapter in self.spine:
            tree = lxml_html.document_fromstring(
                self.zip.read(chapter) or b"<p></p>", parser=_parser()
            )
            for bad in tree.xpath(
                "//script|//style|//iframe|//object|//embed|//link|//noscript|//template|//head"
            ):
                bad.drop_tree()
            for img in tree.xpath("//img|//*[local-name()='image']"):
                ref = (
                    img.get("src")
                    or img.get("href")
                    or img.get("{http://www.w3.org/1999/xlink}href")
                    or ""
                )
                member = self._resolve(chapter, ref)
                data = None
                if embed_images and member and self.media.get(member, "").startswith("image/"):
                    size = self.zip.getinfo(member).file_size
                    if size <= budget:
                        budget -= size
                        data = self.zip.read(member)
                if data is None:
                    img.drop_tree()
                    dropped = True
                elif img.tag == "img":
                    img.set(
                        "src",
                        f"data:{self.media[member]};base64,{base64.b64encode(data).decode('ascii')}",
                    )
                else:
                    # An SVG <image> (cover pages): an <img> works everywhere.
                    new = lxml_html.Element(
                        "img",
                        src=f"data:{self.media[member]};base64,{base64.b64encode(data).decode('ascii')}",
                    )
                    svg = next((a for a in img.iterancestors() if a.tag.endswith("svg")), img)
                    svg.addprevious(new)
                    svg.drop_tree()
            for a in tree.xpath("//a[@href]"):
                href = a.get("href", "")
                target = urlsplit(href)
                if not target.scheme:
                    # Links between chapters: keep the anchor, which is in this file now.
                    if target.fragment:
                        a.set("href", f"#{target.fragment}")
                    else:
                        del a.attrib["href"]
            body = tree.find("body")
            if body is None:
                continue
            inner = (body.text or "") + "".join(
                lxml_html.tostring(child, encoding="unicode") for child in body
            )
            parts.append(f"<section>{inner}</section>")
        return "\n".join(parts), dropped

    def _resolve(self, chapter: str, ref: str) -> str | None:
        target = urlsplit(ref)
        if target.scheme or not target.path:
            return None
        member = posixpath.normpath(
            posixpath.join(posixpath.dirname(chapter), unquote(target.path))
        )
        return member if member in self.names else None


if __name__ == "__main__":
    run_main(MarkupConvertAdapter)
