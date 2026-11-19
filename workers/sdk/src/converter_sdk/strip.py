"""Byte-level removal of metadata from JPEG, PNG and WebP files.

The compressed picture is copied as it is, so pixels never change. PNG and WebP
are cleaned with an allow-list of chunks: a chunk kind nobody has heard of
(a camera's private chunk, a new text chunk) is dropped, where a deny-list
would let it through. Used by ``file-clean`` (pictures, and pictures inside
Office files) and ``media-clean`` (cover art).

Colour profiles are kept, because dropping one changes how the picture looks,
but a profile carries a description (and sometimes a copyright note) that can
name a person or a machine ("Jane's Laptop Display"). The text of those tags
is overwritten in place with a neutral one and the profile ID, a hash that
identifies the exact profile, is zeroed; the profile's size and its colour
tables don't change.
"""

from __future__ import annotations

import re
import zlib
from collections.abc import Callable

from .errors import AdapterError

Tick = Callable[[], None]
"""Called once per segment or chunk, so a long file can be cancelled."""

NEUTRAL_ICC_TEXT = "ICC profile"
MAX_ICC_BYTES = 16 * 1024 * 1024


def _corrupt() -> AdapterError:
    return AdapterError("E_INPUT_CORRUPT")


# -------------------------------------------------------------- ICC profiles


def _u32(data: bytes | bytearray, pos: int) -> int:
    return int.from_bytes(data[pos : pos + 4], "big")


def _blank_text_tag(buf: bytearray, start: int, size: int) -> None:
    """Overwrite the text of one ``desc`` (v2) or ``mluc`` (v4) tag in place."""
    kind = bytes(buf[start : start + 4])
    end = start + size
    if kind == b"desc" and size >= 12:
        count = _u32(buf, start + 8)  # ASCII part, with its NUL
        if 0 < count <= size - 12:
            text = NEUTRAL_ICC_TEXT.encode()[: count - 1].ljust(count, b"\x00")
            buf[start + 12 : start + 12 + count] = text
            # The Unicode and Mac parts after it carry the same name: empty them.
            rest = start + 12 + count
            buf[rest:end] = bytes(end - rest)
    elif kind == b"mluc" and size >= 16:
        records, record_size = _u32(buf, start + 8), _u32(buf, start + 12)
        if record_size < 12 or records > (size - 16) // record_size:
            return
        for i in range(records):
            rec = start + 16 + i * record_size
            length, offset = _u32(buf, rec + 4), _u32(buf, rec + 8)
            if offset + length > size:
                continue
            text = NEUTRAL_ICC_TEXT.encode("utf-16-be")[:length].ljust(length, b"\x00")
            buf[start + offset : start + offset + length] = text


def clean_icc(profile: bytes) -> bytes:
    """``profile`` with its description and copyright text made neutral and its
    ID zeroed. A profile too damaged to read comes back as it was."""
    if len(profile) < 132 or profile[36:40] != b"acsp" or _u32(profile, 0) > len(profile):
        return profile
    buf = bytearray(profile)
    buf[84:100] = bytes(16)
    count = _u32(buf, 128)
    if count > (len(buf) - 132) // 12:
        return profile
    for i in range(count):
        sig = bytes(buf[132 + i * 12 : 136 + i * 12])
        if sig not in (b"desc", b"cprt", b"dmnd", b"dmdd", b"mmod"):
            continue
        offset, size = _u32(buf, 136 + i * 12), _u32(buf, 140 + i * 12)
        if offset + size <= len(buf):
            _blank_text_tag(buf, offset, size)
    return bytes(buf)


def icc_description(profile: bytes) -> str:
    """The description of an ICC profile (for tests and diagnostics)."""
    if len(profile) < 132:
        return ""
    for i in range(min(_u32(profile, 128), 1000)):
        sig = profile[132 + i * 12 : 136 + i * 12]
        if sig != b"desc":
            continue
        start, size = _u32(profile, 136 + i * 12), _u32(profile, 140 + i * 12)
        body = profile[start : start + size]
        if body[:4] == b"desc":
            n = _u32(body, 8)
            return body[12 : 12 + n].split(b"\x00")[0].decode("latin-1")
        if body[:4] == b"mluc":
            length, offset = _u32(body, 20), _u32(body, 24)
            return body[offset : offset + length].decode("utf-16-be", "replace").rstrip("\x00")
    return ""


# -------------------------------------------------------------------- JPEG

_SCAN_MARKER = re.compile(rb"\xff[^\x00\xd0-\xd7\xff]")


def _keep_segment(marker: int, body: bytes) -> bool:
    """Only the segments a picture needs to display: JFIF, ICC and Adobe
    colour flags, and everything that isn't an application or comment segment."""
    if 0xE0 <= marker <= 0xEF:
        return (
            (marker == 0xE0 and body.startswith(b"JFIF\x00"))
            or (marker == 0xE2 and body.startswith(b"ICC_PROFILE\x00"))
            or (marker == 0xEE and body.startswith(b"Adobe"))
        )
    return marker != 0xFE


def clean_jpeg_bytes(data: bytes, tick: Tick | None = None) -> bytes:
    """``data`` without EXIF, XMP, IPTC, thumbnails or comments, and without
    anything after the end of the image. The compressed picture is copied
    byte for byte. (ICC profiles in JPEG are kept as they are.)"""
    n = len(data)
    if data[:2] != b"\xff\xd8":
        raise _corrupt()
    out = bytearray(b"\xff\xd8")
    pos, in_scan = 2, False
    while pos < n:
        if tick:
            tick()
        if in_scan:
            m = _SCAN_MARKER.search(data, pos)
            if m is None:
                raise _corrupt()
            out += data[pos : m.start()]
            pos, in_scan = m.start(), False
        if data[pos] != 0xFF:
            raise _corrupt()
        while pos + 1 < n and data[pos + 1] == 0xFF:
            pos += 1  # fill bytes
        if pos + 1 >= n:
            raise _corrupt()
        marker = data[pos + 1]
        if marker == 0xD9:
            out += b"\xff\xd9"
            return bytes(out)
        if marker in (0x01, 0xD8) or 0xD0 <= marker <= 0xD7:
            out += data[pos : pos + 2]
            pos += 2
            continue
        if pos + 4 > n:
            raise _corrupt()
        length = int.from_bytes(data[pos + 2 : pos + 4], "big")
        end = pos + 2 + length
        if length < 2 or end > n:
            raise _corrupt()
        if _keep_segment(marker, data[pos + 4 : end]):
            out += data[pos:end]
        pos = end
        in_scan = marker == 0xDA
    raise _corrupt()


# --------------------------------------------------------------------- PNG

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
PNG_KEEP = {
    b"IHDR", b"PLTE", b"IDAT", b"IEND",  # what a decoder needs
    b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"cICP", b"pHYs",  # how to show it
    b"acTL", b"fcTL", b"fdAT",  # animation
}  # fmt: skip


def _clean_iccp(body: bytes) -> bytes | None:
    """An ``iCCP`` chunk body with a neutral profile name and description, or
    None if the profile can't be read (the chunk is then dropped)."""
    name_end = body.find(b"\x00")
    if name_end < 1 or len(body) < name_end + 3 or body[name_end + 1] != 0:
        return None
    try:
        inflater = zlib.decompressobj()
        profile = inflater.decompress(body[name_end + 2 :], MAX_ICC_BYTES + 1)
    except zlib.error:
        return None
    if len(profile) > MAX_ICC_BYTES or inflater.unconsumed_tail:
        return None
    return NEUTRAL_ICC_TEXT.encode() + b"\x00\x00" + zlib.compress(clean_icc(profile), 9)


def clean_png_bytes(data: bytes, tick: Tick | None = None) -> bytes:
    """``data`` with only the chunks in ``PNG_KEEP``, in their order."""
    if data[:8] != PNG_SIGNATURE:
        raise _corrupt()
    out = bytearray(PNG_SIGNATURE)
    pos, n = 8, len(data)
    while pos + 12 <= n:
        if tick:
            tick()
        size = int.from_bytes(data[pos : pos + 4], "big")
        kind = data[pos + 4 : pos + 8]
        end = pos + 12 + size
        if end > n:
            raise _corrupt()
        if kind == b"iCCP":
            body = _clean_iccp(data[pos + 8 : pos + 8 + size])
            if body is not None:
                head = b"iCCP" + body
                out += len(body).to_bytes(4, "big") + head + zlib.crc32(head).to_bytes(4, "big")
        elif kind in PNG_KEEP:
            out += data[pos:end]
        elif kind[0] & 0x20 == 0:
            raise _corrupt()  # an unknown critical chunk: the file can't be shown without it
        pos = end
        if kind == b"IEND":
            return bytes(out)
    raise _corrupt()


# -------------------------------------------------------------------- WebP

WEBP_KEEP = {b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP"}
VP8X_METADATA_FLAGS = 0x08 | 0x04  # EXIF, XMP


def clean_webp_bytes(data: bytes, tick: Tick | None = None) -> bytes:
    """``data`` with only the chunks in ``WEBP_KEEP``; the header flags for
    EXIF and XMP are cleared."""
    if data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        raise _corrupt()
    end = 8 + int.from_bytes(data[4:8], "little")
    if end > len(data) or end < 12:
        raise _corrupt()
    chunks = bytearray()
    pos = 12
    while pos + 8 <= end:
        if tick:
            tick()
        fourcc = data[pos : pos + 4]
        size = int.from_bytes(data[pos + 4 : pos + 8], "little")
        body_end = pos + 8 + size
        if body_end > end:
            raise _corrupt()
        if fourcc in WEBP_KEEP:
            body = bytearray(data[pos + 8 : body_end])
            if fourcc == b"VP8X" and body:
                body[0] &= ~VP8X_METADATA_FLAGS & 0xFF
            elif fourcc == b"ICCP":
                body = bytearray(clean_icc(bytes(body)))
            chunks += fourcc + size.to_bytes(4, "little") + body + (b"\x00" if size & 1 else b"")
        pos = body_end + (size & 1)
    return b"RIFF" + (4 + len(chunks)).to_bytes(4, "little") + b"WEBP" + bytes(chunks)


def clean_picture_bytes(data: bytes, tick: Tick | None = None) -> bytes | None:
    """A JPEG, PNG or WebP without its metadata, or None for any other kind of
    data. Identified by content, not by name."""
    if data[:2] == b"\xff\xd8":
        return clean_jpeg_bytes(data, tick)
    if data[:8] == PNG_SIGNATURE:
        return clean_png_bytes(data, tick)
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return clean_webp_bytes(data, tick)
    return None


__all__ = [
    "NEUTRAL_ICC_TEXT",
    "clean_icc",
    "clean_jpeg_bytes",
    "clean_picture_bytes",
    "clean_png_bytes",
    "clean_webp_bytes",
    "icc_description",
]
