"""What a file reveals about its author, its device and its history (ADR 0005).

``findings`` reads one staged input and lists the details a person might not
want to share: where a photo was taken, which camera, who wrote a document,
comments and tracked changes left inside. It is display only; the "Remove
hidden details" tools use it to check their own output too.

Privacy rule: a finding's ``detail`` may name short metadata values (a camera
model, an author, a date, coordinates) but never document text: no comment
text, no body text, no titles.

The module imports no engine at load time: Pillow, pikepdf and lxml are
imported inside the functions that need them.
"""

from __future__ import annotations

import contextlib
import re
import zipfile
from pathlib import Path
from typing import Any

from .context import JobContext
from .convert import source_format
from .errors import AdapterError
from .protocol import Finding

MAX_FINDINGS = 64
MAX_DETAIL = 200
MAX_PART_BYTES = 8 * 1024 * 1024
"""Largest small XML part (properties, workbook) read whole."""
MAX_SCAN_BYTES = 512 * 1024 * 1024
"""Most bytes searched in one Office file, however many parts it has."""
MAX_ENTRIES = 20_000
CHUNK = 1 << 20

IMAGE_FORMATS = {"jpg", "jpeg", "png", "webp", "gif", "tiff", "tif", "avif", "heic", "bmp"}
MEDIA_FORMATS = {"mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "aiff",
                 "mp4", "mov", "mkv", "webm", "avi"}  # fmt: skip
OOXML_FORMATS = {"docx", "xlsx", "pptx"}
MARKUP_FORMATS = {"svg", "html", "htm"}
PRIVACY_KINDS = {"location", "device", "author", "software", "dates", "comments",
                 "trackedChanges", "hiddenContent", "tags"}  # fmt: skip
IDENTIFYING_KINDS = {"location", "device", "author", "dates"}
"""Kinds the cleaning tools must leave empty."""

_CONTROL = re.compile(r"[\x00-\x1f\x7f-\x9f  ]")


def clean_detail(value: Any) -> str:
    """A short, printable form of a metadata value."""
    text = value.decode("utf-8", "replace") if isinstance(value, bytes) else str(value)
    text = _CONTROL.sub(" ", text)
    text = " ".join(text.split())
    return text[:MAX_DETAIL]


def _finding(kind: str, detail: Any = None) -> Finding:
    text = clean_detail(detail) if detail is not None else ""
    return Finding(kind=kind, detail=text or None)  # type: ignore[arg-type]


def _tidy(items: list[Finding]) -> list[Finding]:
    seen: set[tuple[str, str | None]] = set()
    out: list[Finding] = []
    for f in items:
        key = (f.kind, f.detail)
        if key in seen:
            continue
        seen.add(key)
        out.append(f)
        if len(out) >= MAX_FINDINGS:
            break
    return out


# ------------------------------------------------------------------ entry


def findings(ctx: JobContext, path: Path, *, include_active: bool = True) -> list[Finding]:
    """Everything ``path`` reveals, privacy kinds first, then (if
    ``include_active``) what it could do when opened."""
    fmt = source_format(path)
    out: list[Finding] = []
    if fmt in IMAGE_FORMATS:
        out += image_findings(path)
    elif fmt == "pdf":
        pdf = ctx.open_pdf(path)
        try:
            out += pdf_findings(pdf)
            if include_active:
                out += _active_pdf(pdf)
        finally:
            pdf.close()
    elif fmt in OOXML_FORMATS:
        out += ooxml_findings(path)
    elif fmt in MARKUP_FORMATS:
        out += markup_findings(path, fmt)
    elif fmt in MEDIA_FORMATS:
        from .media import probe  # noqa: PLC0415

        out += media_findings(probe(ctx, path))
    if include_active and fmt != "pdf" and fmt not in MEDIA_FORMATS:
        out += _active(path, fmt)
    return _tidy(out)


def open_pdf_findings(pdf: Any) -> list[Finding]:
    """Everything an already opened PDF reveals or could do (for inspect)."""
    return _tidy(pdf_findings(pdf) + _active_pdf(pdf))


def _active(path: Path, fmt: str) -> list[Finding]:
    try:
        from . import active  # noqa: PLC0415
    except ImportError:
        return []
    return list(active.active_findings(path, fmt))


def _active_pdf(pdf: Any) -> list[Finding]:
    try:
        from . import active  # noqa: PLC0415
    except ImportError:
        return []
    return list(active.pdf_active_findings(pdf))


# ----------------------------------------------------------------- images

_EXIF_IFD, _GPS_IFD = 0x8769, 0x8825
_PNG_TEXT_KEYS = {"author": "author", "software": "software", "creation time": "dates"}
_NOT_TEXT = {"dpi", "gamma", "icc_profile", "transparency", "srgb", "interlace", "aspect",
             "chromaticity", "exif", "photoshop", "xmp", "comment", "duration", "loop",
             "background", "version", "extension", "compression", "progressive",
             "progression", "jfif", "jfif_version", "jfif_unit", "jfif_density", "adobe",
             "adobe_transform", "mpoffset", "bits", "default_image", "n_frames",
             "timestamp", "original_size", "scale"}  # fmt: skip


def _ratio(value: Any) -> float:
    return float(value)


def _coordinate(value: Any, ref: Any) -> float | None:
    try:
        d, m, s = (_ratio(v) for v in value)
    except (TypeError, ValueError, ZeroDivisionError):
        return None
    deg = d + m / 60 + s / 3600
    ref = ref.decode("ascii", "ignore") if isinstance(ref, bytes) else str(ref or "")
    return -deg if ref.upper() in ("S", "W") else deg


def _gps(exif: Any) -> Finding | None:
    try:
        gps = exif.get_ifd(_GPS_IFD)
    except Exception:
        gps = None
    if not gps:
        return _finding("location", "GPS position") if exif.get(_GPS_IFD) else None
    lat = _coordinate(gps.get(2), gps.get(1)) if 2 in gps else None
    lon = _coordinate(gps.get(4), gps.get(3)) if 4 in gps else None
    if lat is None or lon is None:
        return _finding("location", "GPS position")
    return _finding("location", f"{lat:.4f}, {lon:.4f}")


def _text(value: Any) -> str:
    if isinstance(value, tuple):  # XPAuthor is UTF-16 bytes in older readers
        value = bytes(value)
    if isinstance(value, bytes):
        if len(value) >= 2 and b"\x00" in value:
            return value.decode("utf-16-le", "ignore").strip("\x00 ")
        return value.decode("utf-8", "ignore").strip("\x00 ")
    return str(value).strip("\x00 ")


def _exif_findings(exif: Any) -> list[Finding]:
    out: list[Finding] = []
    if (gps := _gps(exif)) is not None:
        out.append(gps)
    try:
        sub = exif.get_ifd(_EXIF_IFD)
    except Exception:
        sub = {}
    make, model = _text(exif.get(0x010F, "")), _text(exif.get(0x0110, ""))
    device = " ".join(p for p in (make, model) if p and not model.startswith(make))
    if model and make and model.startswith(make):
        device = model
    if device:
        out.append(_finding("device", device))
    if serial := _text(sub.get(0xA431, "")):
        out.append(_finding("device", f"Camera serial number {serial}"))
    if lens := _text(sub.get(0xA434, "")):
        out.append(_finding("device", f"Lens {lens}"))
    if software := _text(exif.get(0x0131, "")):
        out.append(_finding("software", software))
    for tag, src in ((0x9003, sub), (0x0132, exif)):
        if value := _text(src.get(tag, "")):
            out.append(_finding("dates", value.replace(":", "-", 2)))
            break
    for tag in (0x013B, 0x8298, 0x9C9D):
        if value := _text(exif.get(tag, "")):
            out.append(_finding("author", value))
    return out


def _png_chunk_types(path: Path) -> set[bytes]:
    found: set[bytes] = set()
    try:
        with path.open("rb") as fh:
            if fh.read(8) != b"\x89PNG\r\n\x1a\n":
                return found
            while head := fh.read(8):
                if len(head) < 8:
                    break
                size, kind = int.from_bytes(head[:4], "big"), head[4:]
                found.add(kind)
                if kind == b"IEND":
                    break
                fh.seek(size + 4, 1)
    except OSError:
        pass
    return found


def image_findings(path: Path) -> list[Finding]:
    """EXIF, XMP, IPTC, text chunks and comments of one image."""
    from .images import open_image  # noqa: PLC0415

    img = open_image(path)
    out: list[Finding] = []
    try:
        with contextlib.suppress(Exception):
            out += _exif_findings(img.getexif())
        info = dict(img.info)
        fmt = (img.format or "").upper()
        if info.get("comment"):
            out.append(_finding("comments", "Embedded comment"))
        if info.get("xmp") or info.get("XML:com.adobe.xmp"):
            out.append(_finding("tags", "XMP metadata"))
        if info.get("photoshop") or 33723 in getattr(img, "tag_v2", {}):
            out.append(_finding("tags", "IPTC / Photoshop metadata"))
        if fmt == "TIFF" and 700 in getattr(img, "tag_v2", {}):
            out.append(_finding("tags", "XMP metadata"))
        if fmt == "PNG":
            try:
                texts = getattr(img, "text", {}) or {}
            except (OSError, SyntaxError, ValueError, EOFError):
                texts = {}  # a damaged file: the tool that reads it reports that
            for key in sorted(texts):
                low = str(key).lower()
                if low in _PNG_TEXT_KEYS:
                    out.append(_finding(_PNG_TEXT_KEYS[low], str(texts[key])))
                elif low == "comment":
                    out.append(_finding("comments", "Embedded comment"))
                elif low not in _NOT_TEXT and not low.startswith("xml:"):
                    out.append(_finding("tags", f"Text note: {key}"))
            if b"tIME" in _png_chunk_types(path):
                out.append(_finding("dates", "Last modified time"))
    finally:
        img.close()
    return out


# -------------------------------------------------------------------- PDF

_PDF_DATE = re.compile(r"D?:?(\d{4})(\d\d)?(\d\d)?(\d\d)?(\d\d)?(\d\d)?")


def _pdf_date(value: str) -> str:
    m = _PDF_DATE.match(value.strip())
    if not m or not m.group(1):
        return value
    parts = [p for p in m.groups() if p]
    head = "-".join(parts[:3])
    return head + (" " + ":".join(parts[3:]) if len(parts) > 3 else "")


def pdf_findings(pdf: Any) -> list[Finding]:
    """Document information, XMP, annotations and hidden layers of a PDF."""
    out: list[Finding] = []
    info = pdf.trailer.get("/Info")
    if info is not None and hasattr(info, "keys"):

        def value(key: str) -> str:
            try:
                return str(info.get(key, "") or "")
            except Exception:
                return ""

        if v := value("/Author"):
            out.append(_finding("author", v))
        for key in ("/Creator", "/Producer"):
            if v := value(key):
                out.append(_finding("software", v))
        for key in ("/CreationDate", "/ModDate"):
            if v := value(key):
                out.append(_finding("dates", _pdf_date(v)))
        for key in ("/Subject", "/Keywords"):
            if value(key):
                out.append(_finding("tags", key.lstrip("/")))
    if "/Metadata" in pdf.Root:
        out.append(_finding("tags", "XMP metadata"))
    notes = 0
    for index, page in enumerate(pdf.pages):
        if index >= 5000:
            break
        annots = page.obj.get("/Annots")
        if annots is None:
            continue
        for a in annots:
            with contextlib.suppress(Exception):
                if str(a.get("/Subtype")) not in ("/Link", "/Widget", "/Popup"):
                    notes += 1
    if notes:
        out.append(_finding("comments", f"{notes} annotation{'s' if notes != 1 else ''}"))
    try:
        from .pdf import hidden_ocgs  # noqa: PLC0415

        hidden = hidden_ocgs(pdf)
    except ImportError:
        hidden = set()
    if hidden:
        out.append(
            _finding(
                "hiddenContent", f"{len(hidden)} hidden layer{'s' if len(hidden) != 1 else ''}"
            )
        )
    return out


# ------------------------------------------------------------------ OOXML

_CORE_AUTHORS = ("creator", "lastModifiedBy")
_VANISH = re.compile(rb'<w:vanish(?:\s+w:val="(?:1|true|on)")?\s*/>')
_PRESENTATION_HIDDEN = re.compile(rb'<p:sld\b[^>]*\sshow="(?:0|false)"')


def secure_parser() -> Any:
    from lxml import etree  # noqa: PLC0415

    return etree.XMLParser(resolve_entities=False, no_network=True, load_dtd=False, huge_tree=False)


def parse_xml(data: bytes) -> Any:
    """Parse untrusted XML; a DOCTYPE or malformed markup is corrupt input."""
    from lxml import etree  # noqa: PLC0415

    if b"<!DOCTYPE" in data[:4096].upper() or b"<!ENTITY" in data[:4096].upper():
        raise AdapterError("E_INPUT_CORRUPT")
    try:
        return etree.fromstring(data, secure_parser())
    except etree.XMLSyntaxError:
        raise AdapterError("E_INPUT_CORRUPT") from None


def _local(tag: Any) -> str:
    return tag.rpartition("}")[2] if isinstance(tag, str) else ""


def _children(root: Any) -> dict[str, str]:
    return {_local(el.tag): (el.text or "").strip() for el in root if isinstance(el.tag, str)}


def read_part(z: zipfile.ZipFile, name: str, limit: int = MAX_PART_BYTES) -> bytes | None:
    """One small part, or None if it is missing or too large to read whole."""
    try:
        info = z.getinfo(name)
    except KeyError:
        return None
    if info.file_size > limit:
        return None
    with z.open(info) as fh:
        data = fh.read(limit + 1)
    return data if len(data) <= limit else None


def _stream_search(
    z: zipfile.ZipFile, name: str, pattern: re.Pattern[bytes], budget: list[int]
) -> int:
    """How many matches of ``pattern`` in a part, read in chunks. Matches are
    short, so a 64-byte tail covers the border between chunks."""
    count, tail = 0, b""
    with z.open(name) as fh:
        while chunk := fh.read(CHUNK):
            budget[0] -= len(chunk)
            if budget[0] < 0:
                break
            data = tail + chunk
            count += sum(1 for m in pattern.finditer(data) if m.end() > len(tail))
            tail = data[-64:]
            if count >= 1000:
                break
    return count


def _count_matches(
    z: zipfile.ZipFile, name: str, pattern: re.Pattern[bytes], budget: list[int]
) -> int:
    info = z.getinfo(name)
    if info.file_size <= MAX_PART_BYTES:
        data = read_part(z, name) or b""
        budget[0] -= len(data)
        return len(pattern.findall(data))
    return _stream_search(z, name, pattern, budget)


def _has(z: zipfile.ZipFile, name: str, pattern: re.Pattern[bytes], budget: list[int]) -> bool:
    """Whether ``pattern`` occurs anywhere in a part (chunked, with overlap)."""
    tail = b""
    with z.open(name) as fh:
        while chunk := fh.read(CHUNK):
            budget[0] -= len(chunk)
            if budget[0] < 0:
                return False
            data = tail + chunk
            if pattern.search(data):
                return True
            tail = data[-256:]
    return False


def _plural(n: int, one: str, many: str | None = None) -> str:
    return f"{n} {one}" if n == 1 else f"{n} {many or one + 's'}"


def ooxml_findings(path: Path) -> list[Finding]:
    """Properties, comments, tracked changes and hidden parts of a docx,
    xlsx or pptx. Reads the ZIP without unpacking it."""
    fmt = source_format(path)
    out: list[Finding] = []
    try:
        with zipfile.ZipFile(path) as z:
            names = z.namelist()
            if len(names) > MAX_ENTRIES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "entries"})
            budget = [MAX_SCAN_BYTES]
            out += _ooxml_properties(z, names)
            out += _ooxml_comments(z, names, fmt, budget)
            out += _ooxml_changes(z, names, fmt, budget)
    except (zipfile.BadZipFile, OSError, EOFError, NotImplementedError, RuntimeError):
        raise AdapterError("E_INPUT_CORRUPT") from None
    return out


def _ooxml_properties(z: zipfile.ZipFile, names: list[str]) -> list[Finding]:
    out: list[Finding] = []
    if data := read_part(z, "docProps/core.xml"):
        core = _children(parse_xml(data))
        for key in _CORE_AUTHORS:
            if core.get(key):
                out.append(_finding("author", core[key]))
        for key in ("created", "modified"):
            if core.get(key):
                out.append(_finding("dates", core[key]))
    if data := read_part(z, "docProps/app.xml"):
        app = _children(parse_xml(data))
        software = " ".join(p for p in (app.get("Application"), app.get("AppVersion")) if p)
        if software:
            out.append(_finding("software", software))
        for key in ("Company", "Manager"):
            if app.get(key):
                out.append(_finding("author", app[key]))
        if app.get("Template"):
            out.append(_finding("tags", "Template"))
    if data := read_part(z, "docProps/custom.xml"):
        n = sum(1 for el in parse_xml(data) if isinstance(el.tag, str))
        if n:
            out.append(_finding("tags", _plural(n, "custom property", "custom properties")))
    return out


_COMMENT_PARTS = {
    "docx": (re.compile(r"word/comments\.xml$"), re.compile(rb"<w:comment\s")),
    "xlsx": (re.compile(r"xl/(comments\d*|threadedComments/[^/]+)\.xml$"),
             re.compile(rb"<(?:comment|threadedComment)\s")),
    "pptx": (re.compile(r"ppt/comments/[^/]+\.xml$"),
             re.compile(rb"<(?:p:cm|p188:cm|p:comment)\s")),
}  # fmt: skip


def _ooxml_comments(
    z: zipfile.ZipFile, names: list[str], fmt: str, budget: list[int]
) -> list[Finding]:
    where, element = _COMMENT_PARTS[fmt]
    total = 0
    for name in names:
        if where.match(name):
            total += _count_matches(z, name, element, budget)
    return [_finding("comments", _plural(total, "comment"))] if total else []


def _ooxml_changes(
    z: zipfile.ZipFile, names: list[str], fmt: str, budget: list[int]
) -> list[Finding]:
    out: list[Finding] = []
    if fmt == "docx":
        tracked = re.compile(rb"<w:(?:ins|del|moveFrom|moveTo)\s")
        parts = [n for n in names if re.fullmatch(r"word/[^/]+\.xml", n)]
        if any(_has(z, n, tracked, budget) for n in parts):
            out.append(_finding("trackedChanges", "Insertions or deletions"))
        if any(_has(z, n, _VANISH, budget) for n in parts):
            out.append(_finding("hiddenContent", "Hidden text"))
    elif fmt == "xlsx":
        if data := read_part(z, "xl/workbook.xml"):
            states = [
                el.get("state")
                for el in parse_xml(data).iter()
                if _local(el.tag) == "sheet" and el.get("state") in ("hidden", "veryHidden")
            ]
            if states:
                out.append(_finding("hiddenContent", _plural(len(states), "hidden sheet")))
    elif fmt == "pptx":
        slides = [n for n in names if re.fullmatch(r"ppt/slides/slide\d+\.xml", n)]
        hidden = 0
        for n in slides:
            with z.open(n) as fh:
                head = fh.read(8192)
            hidden += bool(_PRESENTATION_HIDDEN.search(head))
        if hidden:
            out.append(_finding("hiddenContent", _plural(hidden, "hidden slide")))
    return out


# ----------------------------------------------------------------- markup

_MARKUP_LIMIT = 4 * 1024 * 1024
_META = re.compile(rb"<meta\s[^>]*>", re.I)
_ATTR = re.compile(rb'([a-zA-Z:-]+)\s*=\s*(?:"([^"]*)"|\'([^\']*)\')')


def markup_findings(path: Path, fmt: str) -> list[Finding]:
    """Privacy details of an SVG or HTML file, found without a full parse."""
    try:
        with path.open("rb") as fh:
            data = fh.read(_MARKUP_LIMIT)
    except OSError:
        raise AdapterError("E_INPUT_CORRUPT") from None
    out: list[Finding] = []
    if fmt == "svg":
        if re.search(rb"<(?:\w+:)?metadata[\s>]", data):
            out.append(_finding("tags", "Metadata block"))
        if re.search(rb"sodipodi:docname\s*=|inkscape:export-filename\s*=", data):
            out.append(_finding("tags", "Editor file names"))
        if m := re.search(rb'inkscape:version\s*=\s*"([^"]{1,60})"', data):
            out.append(_finding("software", "Inkscape " + m.group(1).decode("utf-8", "replace")))
        return out
    for tag in _META.findall(data):
        attrs = {
            k.decode("ascii", "ignore").lower(): (v1 or v2).decode("utf-8", "replace")
            for k, v1, v2 in _ATTR.findall(tag)
        }
        name = attrs.get("name", "").lower()
        if name == "author" and attrs.get("content"):
            out.append(_finding("author", attrs["content"]))
        elif name == "generator" and attrs.get("content"):
            out.append(_finding("software", attrs["content"]))
    return out


# ------------------------------------------------------------------ media

_ISO6709 = re.compile(r"([+-]\d+(?:\.\d+)?)([+-]\d+(?:\.\d+)?)")
_DEVICE_MAKE = {"make", "com.apple.quicktime.make", "com.android.manufacturer"}
_DEVICE_MODEL = {"model", "com.apple.quicktime.model", "com.android.model"}
_SOFTWARE = {"encoder", "software", "com.apple.quicktime.software", "com.android.version"}
_DATES = {"creation_time", "date", "com.apple.quicktime.creationdate", "creationdate"}
_AUTHORS = {"artist", "album_artist", "composer", "author", "copyright", "performer"}
_TAGS = ("title", "album", "comment", "genre", "description")


def _tags_of(info: dict[str, Any]) -> list[dict[str, str]]:
    sources = [info.get("format") or {}, *(info.get("streams") or [])]
    result = []
    for src in sources:
        tags = src.get("tags") or {}
        result.append({str(k).lower(): str(v) for k, v in tags.items()})
    return result


def media_findings(info: dict[str, Any]) -> list[Finding]:
    """Findings from ffprobe's JSON (format and stream tags, cover pictures)."""
    out: list[Finding] = []
    tag_names: list[str] = []
    for tags in _tags_of(info):
        make = next((tags[k] for k in _DEVICE_MAKE if tags.get(k)), "")
        model = next((tags[k] for k in _DEVICE_MODEL if tags.get(k)), "")
        if make or model:
            out.append(_finding("device", " ".join(p for p in (make, model) if p)))
        for key, value in tags.items():
            if not value.strip():
                continue
            if key.startswith("location") or "iso6709" in key:
                m = _ISO6709.match(value.strip())
                out.append(
                    _finding("location", f"{float(m.group(1)):.4f}, {float(m.group(2)):.4f}")
                    if m
                    else _finding("location", "GPS position")
                )
            elif key in _SOFTWARE:
                if not value.startswith(("Lavf", "Lavc")):
                    out.append(_finding("software", value))
            elif key in _DATES:
                out.append(_finding("dates", value))
            elif key in _AUTHORS:
                out.append(_finding("author", value))
            elif key in _TAGS and key not in tag_names:
                tag_names.append(key)
    if tag_names:
        out.append(_finding("tags", ", ".join(n.capitalize() for n in tag_names)))
    for stream in info.get("streams") or []:
        if (stream.get("disposition") or {}).get("attached_pic"):
            out.append(_finding("tags", "Cover picture"))
            break
    return _tidy(out)


__all__ = [
    "IDENTIFYING_KINDS",
    "clean_detail",
    "findings",
    "image_findings",
    "markup_findings",
    "media_findings",
    "ooxml_findings",
    "parse_xml",
    "pdf_findings",
    "read_part",
]
