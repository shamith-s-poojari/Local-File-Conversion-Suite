"""The one way to parse XML that came from a user's file.

No entities are expanded, no DTD is loaded and nothing is fetched. A DOCTYPE
is refused: Office parts and the XML formats the tools read never need one, and
a DOCTYPE is where entity bombs and external entities live. A caller that must
accept one fixed, known DOCTYPE (the standard SVG one) says so with
``doctype_ok``; that decision stays with the caller, which knows its format.

lxml is imported inside the function, like the other engines, so importing the
SDK stays cheap.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from .errors import AdapterError


def parse_xml(
    data: bytes, *, huge: bool = False, doctype_ok: Callable[[bytes], bool] | None = None
) -> Any:
    """The root element of ``data``. Malformed XML, an entity declaration or a
    DOCTYPE is ``E_INPUT_CORRUPT``.

    ``huge`` lifts libxml2's limits on depth and text-node size, for a part the
    caller has already bounded by size. ``doctype_ok(data)`` may allow a
    DOCTYPE the caller has checked byte for byte."""
    from lxml import etree  # noqa: PLC0415

    parser = etree.XMLParser(
        resolve_entities=False, no_network=True, load_dtd=False, huge_tree=huge
    )
    try:
        root = etree.fromstring(data, parser=parser)
    except etree.XMLSyntaxError:
        raise AdapterError("E_INPUT_CORRUPT") from None
    # Checked on the parsed tree, not on the bytes: a UTF-16 document or a long
    # comment in front would hide a DOCTYPE from a byte search.
    if root is None or b"<!ENTITY" in data[:100_000]:
        raise AdapterError("E_INPUT_CORRUPT")
    if root.getroottree().docinfo.doctype and not (doctype_ok is not None and doctype_ok(data)):
        raise AdapterError("E_INPUT_CORRUPT")
    return root
