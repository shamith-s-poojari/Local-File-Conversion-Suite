"""The shared XML parser refuses what makes untrusted XML dangerous."""

from __future__ import annotations

import time

import pytest
from converter_sdk import active, reveal, xmlsafe
from converter_sdk.errors import AdapterError

BOMB = (
    b'<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol">'
    b'<!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">'
    b'<!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">'
    b'<!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;">'
    b"]><root>&lol4;</root>"
)
EXTERNAL = b'<?xml version="1.0"?><!DOCTYPE r [<!ENTITY x SYSTEM "file:///etc/passwd">]><r>&x;</r>'


def refused(data: bytes, **kw: object) -> bool:
    try:
        xmlsafe.parse_xml(data, **kw)  # type: ignore[arg-type]
    except AdapterError as e:
        return e.code == "E_INPUT_CORRUPT"
    return False


def test_plain_xml_parses() -> None:
    root = xmlsafe.parse_xml(b'<a xmlns:x="urn:x"><b x:k="1">t</b></a>')
    assert root[0].text == "t"


def test_entity_bomb_is_refused_without_expanding() -> None:
    start = time.monotonic()
    assert refused(BOMB)
    assert time.monotonic() - start < 2


def test_external_entity_is_refused() -> None:
    assert refused(EXTERNAL)


def test_any_doctype_is_refused_even_one_a_byte_search_would_miss() -> None:
    assert refused(b"<!DOCTYPE r><r/>")
    assert refused(b"<!-- " + b"x" * 10_000 + b" --><!DOCTYPE r><r/>")
    utf16 = '<?xml version="1.0" encoding="UTF-16"?><!DOCTYPE r><r/>'.encode("utf-16")
    assert refused(utf16)


def test_malformed_xml_and_undeclared_entities_are_corrupt() -> None:
    assert refused(b"<a><b></a>")
    assert refused(b"<a>&nope;</a>")
    assert refused(b"")


def test_a_doctype_the_caller_vouches_for_is_allowed() -> None:
    data = b"<!DOCTYPE r><r/>"
    assert xmlsafe.parse_xml(data, doctype_ok=lambda d: d == data).tag == "r"
    assert refused(data, doctype_ok=lambda d: False)
    # An entity declaration is refused whatever the caller says.
    assert refused(EXTERNAL, doctype_ok=lambda d: True)


def test_huge_lifts_only_the_depth_limit() -> None:
    deep = b"<a>" * 400 + b"</a>" * 400
    assert refused(deep)
    assert xmlsafe.parse_xml(deep, huge=True).tag == "a"


def test_the_older_entry_points_use_it() -> None:
    assert reveal.parse_xml is xmlsafe.parse_xml
    with pytest.raises(AdapterError):
        active.parse_xml(EXTERNAL)
    svg = (
        b'<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" '
        b'"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd"><svg/>'
    )
    assert active.parse_xml(svg, svg=True).tag == "svg"
    with pytest.raises(AdapterError):
        active.parse_xml(svg)
