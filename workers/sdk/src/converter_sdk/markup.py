"""Markdown, HTML and plain text (ADR 0003).

``markdown_to_html`` uses markdown-it (CommonMark plus tables and
strikethrough; raw HTML in the source is escaped, never passed through).
``html_to_markdown`` walks an lxml tree: headings, paragraphs, emphasis,
code, links, lists, quotes and tables become Markdown; scripts, styles and
images are dropped. Neither touches the network.
"""

from __future__ import annotations

import html as html_lib
import re
from pathlib import Path
from typing import Any

PAGE_CSS = """
body { font-family: Helvetica, Arial, sans-serif; font-size: 11pt; line-height: 1.45;
  color: #1f1e1c; }
h1 { font-size: 22pt; margin: 0 0 10pt; } h2 { font-size: 16pt; margin: 14pt 0 6pt; }
h3 { font-size: 13pt; margin: 12pt 0 4pt; } p { margin: 0 0 8pt; }
code, pre { font-family: Courier, monospace; font-size: 9.5pt; }
pre { background: #f3f2ef; padding: 6pt; }
blockquote { margin: 0 0 8pt 12pt; color: #4a4843; }
table { border-collapse: collapse; margin: 0 0 8pt; }
th, td { border: 1px solid #b9b6af; padding: 3pt 6pt; text-align: left; }
"""


def read_text(path: Path) -> str:
    """A text file's contents: UTF-8 (with or without a BOM), UTF-16 with a
    BOM, the charset an HTML file declares, else Windows-1252."""
    data = path.read_bytes()
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        return data.decode("utf-16", errors="replace")
    try:
        return data.decode("utf-8-sig")
    except UnicodeDecodeError:
        pass
    declared = re.search(rb"<meta[^>]+charset=[\"']?([A-Za-z0-9_.:-]{1,40})", data[:4096], re.I)
    if declared:
        try:
            return data.decode(declared.group(1).decode("ascii"))
        except (LookupError, UnicodeDecodeError):
            pass
    return data.decode("cp1252", errors="replace")


def markdown_to_html(text: str, title: str = "Document") -> str:
    from markdown_it import MarkdownIt  # noqa: PLC0415 - engine imported on demand

    md = MarkdownIt("commonmark", {"html": False, "linkify": False}).enable(
        ["table", "strikethrough"]
    )
    return html_document(md.render(text), title)


def html_document(body: str, title: str = "Document") -> str:
    """A standalone page around ``body`` (already escaped HTML)."""
    return (
        '<!DOCTYPE html>\n<html><head><meta charset="utf-8">'
        f"<title>{html_lib.escape(title)}</title><style>{PAGE_CSS}</style></head>"
        f"<body>\n{body.rstrip()}\n</body></html>\n"
    )


def text_to_html(text: str, title: str = "Document") -> str:
    paragraphs = [p for p in re.split(r"\n\s*\n", text.replace("\r\n", "\n")) if p.strip()]
    body = "\n".join(f"<p>{html_lib.escape(p).replace(chr(10), '<br>')}</p>" for p in paragraphs)
    return html_document(body, title)


_MD_SPECIAL = re.compile(r"([\\`*_{}\[\]<>#|])")


def escape_markdown(text: str) -> str:
    out = _MD_SPECIAL.sub(r"\\\1", text)
    # A line that would start a list or heading by accident.
    return re.sub(r"^(\s*)([-+]|\d+\.)(\s)", r"\1\\\2\3", out, flags=re.M)


def text_to_markdown(text: str) -> str:
    paragraphs = [
        p.strip("\n") for p in re.split(r"\n\s*\n", text.replace("\r\n", "\n")) if p.strip()
    ]
    return "\n\n".join(escape_markdown(p).replace("\n", "  \n") for p in paragraphs) + "\n"


def markdown_to_text(text: str) -> str:
    """The words of a Markdown document, without its markup."""
    from lxml import html as lxml_html  # noqa: PLC0415

    tree = lxml_html.document_fromstring(markdown_to_html(text), parser=_parser())
    return html_tree_to_text(tree)


def html_tree_to_text(tree: Any) -> str:
    for bad in tree.xpath("//script|//style|//head"):
        bad.drop_tree()
    blocks = []
    for el in tree.iter("p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "pre", "blockquote", "tr"):
        if el.tag == "tr":
            text = "\t".join(c.text_content().strip() for c in el if c.tag in ("td", "th"))
        elif el.tag == "blockquote" or el.getparent() is not None and el.getparent().tag == "li":
            continue
        else:
            text = el.text_content().strip()
        if text:
            blocks.append(text)
    return "\n\n".join(blocks) + "\n"


def _strip_declaration(source: str) -> str:
    """lxml refuses text that starts with ``<?xml … encoding=…?>`` (XHTML
    from LibreOffice): it's already decoded, so the declaration can go."""
    return re.sub(r"^\ufeff?\s*<\?xml[^>]*\?>", "", source)


def parse_html(source: str) -> Any:
    """An lxml tree of an HTML document: no network, no comments."""
    from lxml import html as lxml_html  # noqa: PLC0415

    return lxml_html.document_fromstring(_strip_declaration(source) or "<p></p>", parser=_parser())


def html_to_text(source: str) -> str:
    return html_tree_to_text(parse_html(source))


def _parser() -> Any:
    from lxml import html as lxml_html  # noqa: PLC0415

    return lxml_html.HTMLParser(no_network=True, remove_comments=True, remove_pis=True)


def html_to_markdown(source: str) -> tuple[str, bool]:
    """Markdown for an HTML document, and whether images were dropped."""

    tree = parse_html(source)
    for bad in tree.xpath("//script|//style|//noscript|//template|//head"):
        bad.drop_tree()
    state = {"images": False}
    body = tree.find("body")
    out = _blocks(body if body is not None else tree, state, depth=0)
    text = re.sub(r"\n{3,}", "\n\n", out).strip() + "\n"
    return text, state["images"]


def _inline(el: Any, state: dict[str, bool]) -> str:
    parts = [escape_markdown(el.text or "")] if el.text else []
    for child in el:
        parts.append(_inline_element(child, state))
        if child.tail:
            parts.append(escape_markdown(child.tail))
    return "".join(parts)


def _inline_element(el: Any, state: dict[str, bool]) -> str:
    tag = el.tag if isinstance(el.tag, str) else ""
    inner = _inline(el, state)
    if tag in ("strong", "b"):
        return f"**{inner.strip()}**" if inner.strip() else ""
    if tag in ("em", "i"):
        return f"*{inner.strip()}*" if inner.strip() else ""
    if tag in ("s", "del", "strike"):
        return f"~~{inner.strip()}~~" if inner.strip() else ""
    if tag == "code":
        return f"`{el.text_content()}`"
    if tag == "br":
        return "  \n"
    if tag == "a":
        href = (el.get("href") or "").strip()
        if href and not href.lower().startswith(("javascript:", "data:")):
            return f"[{inner.strip() or href}]({href})"
        return inner
    if tag == "img":
        state["images"] = True
        return ""
    return inner


BLOCK = {"p", "div", "section", "article", "main", "header", "footer",
         "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "pre", "blockquote",
         "table", "hr", "figure", "dl", "body", "center"}  # fmt: skip


def _blocks(el: Any, state: dict[str, bool], depth: int) -> str:
    out: list[str] = []
    pending = escape_markdown(el.text or "").strip() if el.text else ""
    for child in el:
        tag = child.tag if isinstance(child.tag, str) else ""
        if tag not in BLOCK:
            pending += _inline_element(child, state)
            if child.tail:
                pending += escape_markdown(child.tail)
            continue
        if pending.strip():
            out.append(pending.strip())
        pending = escape_markdown(child.tail or "").strip() if child.tail else ""
        out.append(_block(child, tag, state, depth))
    if pending.strip():
        out.append(pending.strip())
    return "\n\n".join(b for b in out if b)


def _block(el: Any, tag: str, state: dict[str, bool], depth: int) -> str:
    if tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
        text = _inline(el, state).strip()
        return f"{'#' * int(tag[1])} {text}" if text else ""
    if tag == "p":
        return _inline(el, state).strip()
    if tag == "pre":
        return "```\n" + el.text_content().rstrip("\n") + "\n```"
    if tag == "hr":
        return "---"
    if tag == "blockquote":
        inner = _blocks(el, state, depth)
        return "\n".join(f"> {line}" if line else ">" for line in inner.splitlines())
    if tag in ("ul", "ol"):
        return _list(el, tag == "ol", state, depth)
    if tag == "table":
        return _table(el, state)
    return _blocks(el, state, depth)


def _list(el: Any, ordered: bool, state: dict[str, bool], depth: int) -> str:
    lines = []
    n = 1
    for li in el:
        if not isinstance(li.tag, str) or li.tag != "li":
            continue
        marker = f"{n}." if ordered else "-"
        n += 1
        nested = [c for c in li if isinstance(c.tag, str) and c.tag in ("ul", "ol")]
        for c in nested:
            li.remove(c)
        text = _blocks(li, state, depth + 1).replace("\n\n", " ").strip()
        lines.append(f"{'  ' * depth}{marker} {text}")
        for c in nested:
            lines.append(_list(c, c.tag == "ol", state, depth + 1))
    return "\n".join(lines)


def _table(el: Any, state: dict[str, bool]) -> str:
    rows = []
    for tr in el.iter("tr"):
        cells = [
            _inline(c, state).replace("\n", " ").strip()
            for c in tr
            if isinstance(c.tag, str) and c.tag in ("td", "th")
        ]
        if cells:
            rows.append(cells)
    if not rows:
        return ""
    width = max(len(r) for r in rows)
    rows = [r + [""] * (width - len(r)) for r in rows]
    lines = ["| " + " | ".join(rows[0]) + " |", "|" + "---|" * width]
    lines += ["| " + " | ".join(r) + " |" for r in rows[1:]]
    return "\n".join(lines)
