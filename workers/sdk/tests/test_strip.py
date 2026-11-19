"""Byte-level cleaning of JPEG, PNG and WebP: allow-lists, ICC text, cancel."""

from __future__ import annotations

import io
import zlib

import pytest
from converter_sdk import strip
from converter_sdk.errors import AdapterError
from PIL import Image


def chunk(kind: bytes, body: bytes) -> bytes:
    return len(body).to_bytes(4, "big") + kind + body + zlib.crc32(kind + body).to_bytes(4, "big")


def png(extra: list[bytes]) -> bytes:
    buf = io.BytesIO()
    Image.new("RGB", (4, 4), (1, 2, 3)).save(buf, "PNG")
    data = buf.getvalue()
    return data[:33] + b"".join(extra) + data[33:]


def kinds(data: bytes) -> list[bytes]:
    pos, out = 8, []
    while pos < len(data):
        out.append(data[pos + 4 : pos + 8])
        pos += 12 + int.from_bytes(data[pos : pos + 4], "big")
    return out


def icc(desc: str, version: int = 2) -> bytes:
    if version == 2:
        text = desc.encode() + b"\x00"
        tag = b"desc" + bytes(4) + len(text).to_bytes(4, "big") + text + bytes(78)
    else:
        text = desc.encode("utf-16-be")
        record = b"enUS" + len(text).to_bytes(4, "big") + (28).to_bytes(4, "big")
        tag = b"mluc" + bytes(4) + (1).to_bytes(4, "big") + (12).to_bytes(4, "big") + record + text
    head = bytearray(128)
    head[0:4] = (128 + 16 + len(tag)).to_bytes(4, "big")
    head[36:40] = b"acsp"
    head[84:100] = b"\x42" * 16
    table = (
        (1).to_bytes(4, "big") + b"desc" + (144).to_bytes(4, "big") + len(tag).to_bytes(4, "big")
    )
    return bytes(head) + table + tag


@pytest.mark.parametrize("version", [2, 4])
def test_icc_text_is_made_neutral_in_place(version: int) -> None:
    profile = icc("Jane's Laptop Display", version)
    assert strip.icc_description(profile) == "Jane's Laptop Display"
    clean = strip.clean_icc(profile)
    assert len(clean) == len(profile)
    assert strip.icc_description(clean) == "ICC profile"
    assert clean[84:100] == bytes(16) and clean[:84] == profile[:84]
    assert b"Jane" not in clean


def test_a_damaged_icc_profile_comes_back_unchanged() -> None:
    assert strip.clean_icc(b"short") == b"short"
    assert strip.clean_icc(bytes(200)) == bytes(200)


def test_png_allow_list_drops_unknown_ancillary_chunks() -> None:
    data = png([chunk(b"tEXt", b"a\x00b"), chunk(b"prVt", b"x"), chunk(b"tIME", bytes(7)),
                chunk(b"eXIf", b"MM"), chunk(b"sRGB", b"\x00"), chunk(b"zzZz", b"y")])  # fmt: skip
    assert kinds(strip.clean_png_bytes(data)) == [b"IHDR", b"sRGB", b"IDAT", b"IEND"]


def test_png_unknown_critical_chunk_and_bad_iccp() -> None:
    with pytest.raises(AdapterError):
        strip.clean_png_bytes(png([chunk(b"ABCD", b"x")]))
    # A profile that can't be read is dropped rather than kept with its name.
    out = strip.clean_png_bytes(png([chunk(b"iCCP", b"Jane\x00\x00not zlib")]))
    assert b"iCCP" not in out and b"Jane" not in out


def test_jpeg_loop_and_png_loop_tick() -> None:
    buf = io.BytesIO()
    Image.new("RGB", (8, 8)).save(buf, "JPEG")
    for fn, data in ((strip.clean_jpeg_bytes, buf.getvalue()), (strip.clean_png_bytes, png([]))):
        calls = []
        fn(data, lambda calls=calls: calls.append(1))
        assert len(calls) >= 3

    class Stop(Exception):
        pass

    def stop() -> None:
        raise Stop

    with pytest.raises(Stop):
        strip.clean_png_bytes(png([]), stop)


def test_webp_loop_ticks_and_unknown_chunks_go() -> None:
    body = b"WEBP" + b"VP8L" + (5).to_bytes(4, "little") + b"\x2f\x00\x00\x00\x00" + b"\x00"
    body += b"JUNK" + (2).to_bytes(4, "little") + b"ab"
    data = b"RIFF" + len(body).to_bytes(4, "little") + body
    calls = []
    out = strip.clean_webp_bytes(data, lambda: calls.append(1))
    assert len(calls) == 2 and b"JUNK" not in out and b"VP8L" in out
