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
import io
import re
import warnings
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
MAX_MEDIA_BYTES = 64 * 1024 * 1024
"""Largest picture inside an Office file or PDF that is read for metadata. The
cleaning tool refuses a larger one rather than leave it unchecked."""
MAX_PACKAGE_BYTES = 8 * 1024 * 1024 * 1024

IMAGE_FORMATS = {"jpg", "jpeg", "png", "webp", "gif", "tiff", "tif", "avif", "heic", "bmp"}
MEDIA_FORMATS = {"mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "aiff",
                 "mp4", "mov", "mkv", "webm", "avi"}  # fmt: skip
OOXML_FORMATS = {"docx", "xlsx", "pptx"}
MARKUP_FORMATS = {"svg", "html", "htm"}
PRIVACY_KINDS = {"location", "device", "author", "software", "dates", "comments",
                 "trackedChanges", "hiddenContent", "tags"}  # fmt: skip
IDENTIFYING_KINDS = {"location", "device", "author", "dates", "externalLinks"}
"""Kinds the cleaning tools must leave empty. ``externalLinks`` is only ever
reported for Office files, where a link to a template or file outside the
document carries the path (and so the user name) of whoever made it."""

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
        # With ``include_active`` the active-content reader reports outside links.
        out += ooxml_findings(path, external=not include_active)
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


def _png_chunk_stream(fh: Any) -> set[bytes]:
    found: set[bytes] = set()
    try:
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


def _png_chunk_types(path: Path) -> set[bytes]:
    try:
        with path.open("rb") as fh:
            return _png_chunk_stream(fh)
    except OSError:
        return set()


def _picture_findings(img: Any, png_chunks: Any) -> list[Finding]:
    """The findings of one opened picture; ``png_chunks()`` gives its PNG chunk types."""
    out: list[Finding] = []
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
        if b"tIME" in png_chunks():
            out.append(_finding("dates", "Last modified time"))
    return out


def image_findings(path: Path) -> list[Finding]:
    """EXIF, XMP, IPTC, text chunks and comments of one image."""
    from .images import open_image  # noqa: PLC0415

    img = open_image(path)
    try:
        return _picture_findings(img, lambda: _png_chunk_types(path))
    finally:
        img.close()


def image_bytes_findings(data: bytes) -> list[Finding]:
    """The same for a picture held in memory (one inside an Office file or a
    PDF). A picture Pillow can't read has nothing it can show, so no findings."""
    from PIL import Image  # noqa: PLC0415

    try:
        with warnings.catch_warnings():
            warnings.simplefilter("error", Image.DecompressionBombWarning)
            img = Image.open(io.BytesIO(data))
    except Exception:  # noqa: BLE001 - any unreadable picture
        return []
    try:
        return _picture_findings(img, lambda: _png_chunk_stream(io.BytesIO(data)))
    except Exception:  # noqa: BLE001
        return []
    finally:
        img.close()


# -------------------------------------------------------------------- PDF

_PDF_DATE = re.compile(r"D?:?(\d{4})(\d\d)?(\d\d)?(\d\d)?(\d\d)?(\d\d)?")


def _pdf_date(value: str) -> str:
    m = _PDF_DATE.match(value.strip())
    if not m or not m.group(1):
        return value
    parts = [p for p in m.groups() if p]
    head = "-".join(parts[:3])
    return head + (" " + ":".join(parts[3:]) if len(parts) > 3 else "")


def dct_image_bytes(obj: Any) -> bytes | None:
    """The JPEG held by a PDF image stream (``/DCTDecode``, alone or after
    ``/FlateDecode``), or None for any other kind of stream or one over the limit."""
    import zlib  # noqa: PLC0415

    import pikepdf  # noqa: PLC0415

    try:
        if obj.get("/Subtype") != "/Image":
            return None
        filters = obj.get("/Filter")
        if filters is None:
            return None
        names = [str(f) for f in filters] if isinstance(filters, pikepdf.Array) else [str(filters)]
        if names not in (["/DCTDecode"], ["/FlateDecode", "/DCTDecode"]):
            return None
        if int(obj.get("/Length", 0)) > MAX_MEDIA_BYTES:
            return None
        raw = obj.read_raw_bytes()
        if names[0] == "/FlateDecode":
            inflater = zlib.decompressobj()
            raw = inflater.decompress(raw, MAX_MEDIA_BYTES + 1)
        return raw if len(raw) <= MAX_MEDIA_BYTES and raw[:2] == b"\xff\xd8" else None
    except Exception:  # noqa: BLE001 - a damaged stream has no picture to show
        return None


def _pdf_picture_findings(pdf: Any) -> list[Finding]:
    """Metadata inside the JPEG pictures of a PDF: a viewer never shows it, but
    the stream is the original file."""
    import pikepdf  # noqa: PLC0415

    out: list[Finding] = []
    budget = MAX_SCAN_BYTES
    for obj in pdf.objects:
        if not isinstance(obj, pikepdf.Stream):
            continue
        data = dct_image_bytes(obj)
        if data is None:
            continue
        budget -= len(data)
        if budget < 0:
            break
        out += image_bytes_findings(data)
    return out


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
    out += _pdf_picture_findings(pdf)
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


MEDIA_PART = re.compile(r"(?:word|xl|ppt)/media/[^/]+\.(?:jpe?g|png|webp|tiff?)", re.I)
_DATE_EXTRAS = {0x5455, 0x000A, 0x5855}
"""ZIP extra-field ids that carry a modification time (extended timestamp, NTFS, old Unix)."""
FIXED_ZIP_DATE = (1980, 1, 1, 0, 0, 0)
TRACKED_PLACEHOLDER = "Author"
"""What the cleaning tool writes as the author of a tracked change (Word's own choice)."""
_ATTRS = rb"""(?:\s+[^\s=<>/"']+\s*=\s*(?:"[^"<]*"|'[^'<]*'))*"""
TRACKED_TAG = re.compile(
    rb"<(?:[\w.-]+:)?(?:ins|del|moveFrom|moveTo|moveFromRangeStart|moveToRangeStart|cellIns"
    rb"|cellDel|cellMerge|rPrChange|pPrChange|sectPrChange|tblPrChange|trPrChange|tcPrChange"
    rb"|tblGridChange|numberingChange)" + _ATTRS + rb"\s*/?>"
)
"""A tracked-change element with all its attributes. Attribute values can't hold
``<``, so a tag that never closes costs one scan to the next ``<``."""
TRACKED_ATTR = re.compile(
    rb"""(\s+)((?:[\w.-]+:)?)(author|date|dateUtc)(\s*=\s*)("[^"<]*"|'[^'<]*')"""
)
TRACKED_CARRY = 16 * 1024


def ooxml_findings(path: Path, *, external: bool = True) -> list[Finding]:
    """Properties, comments, tracked changes, hidden parts, pictures, package
    dates and outside links of a docx, xlsx or pptx. Reads the ZIP without
    unpacking it. ``external`` is False when the caller gets outside links
    from ``converter_sdk.active``."""
    from .markup import check_zip  # noqa: PLC0415

    fmt = source_format(path)
    out: list[Finding] = []
    try:
        with zipfile.ZipFile(path) as z:
            infos = check_zip(z, member_max=MAX_PACKAGE_BYTES, total_max=MAX_PACKAGE_BYTES)
            names = [i.filename for i in infos]
            if len(names) > MAX_ENTRIES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "entries"})
            budget = [MAX_SCAN_BYTES]
            out += _ooxml_properties(z, names)
            out += _ooxml_comments(z, names, fmt, budget)
            out += _ooxml_changes(z, names, fmt, budget)
            out += _ooxml_pictures(z, infos, budget)
            out += _ooxml_package(infos)
            if external:
                out += _ooxml_outside_links(z, names)
    except (zipfile.BadZipFile, OSError, EOFError, NotImplementedError, RuntimeError):
        raise AdapterError("E_INPUT_CORRUPT") from None
    return out


def _ooxml_pictures(
    z: zipfile.ZipFile, infos: list[zipfile.ZipInfo], budget: list[int]
) -> list[Finding]:
    """What the pictures in ``word|xl|ppt/media`` carry: they are the original
    files, EXIF and all."""
    out: list[Finding] = []
    for info in infos:
        if not MEDIA_PART.fullmatch(info.filename) or info.file_size > MAX_MEDIA_BYTES:
            continue
        budget[0] -= info.file_size
        if budget[0] < 0:
            break
        with z.open(info) as fh:
            out += image_bytes_findings(fh.read(MAX_MEDIA_BYTES + 1))
    return out


def _has_time_extra(extra: bytes) -> bool:
    pos = 0
    while pos + 4 <= len(extra):
        ident = int.from_bytes(extra[pos : pos + 2], "little")
        if ident in _DATE_EXTRAS:
            return True
        pos += 4 + int.from_bytes(extra[pos + 2 : pos + 4], "little")
    return False


def _ooxml_package(infos: list[zipfile.ZipInfo]) -> list[Finding]:
    """When the parts were last saved, which the ZIP records for each of them."""
    for info in infos:
        if info.is_dir():
            continue
        if info.date_time != FIXED_ZIP_DATE or _has_time_extra(info.extra):
            return [_finding("dates", "Save times of the parts")]
    return []


def _ooxml_outside_links(z: zipfile.ZipFile, names: list[str]) -> list[Finding]:
    """Relationships that point outside the file other than plain web links: a
    template, a linked picture or file. Their targets are paths and addresses
    of the author. Links to other workbooks are the exception: the formulas
    need them, so the cleaning tool keeps them and so doesn't look for them."""
    from .active import is_web_url  # noqa: PLC0415

    total = 0
    for name in names:
        if not name.endswith(".rels") or name.lower().startswith("xl/externallinks/"):
            continue
        data = read_part(z, name)
        if data is None:
            continue
        for rel in parse_xml(data):
            if _local(rel.tag) != "Relationship":
                continue
            if (rel.get("TargetMode") or "").lower() != "external":
                continue
            hyperlink = (rel.get("Type") or "").lower().endswith("/hyperlink")
            if not (hyperlink and is_web_url(rel.get("Target") or "")):
                total += 1
    if not total:
        return []
    return [_finding("externalLinks", f"{_plural(total, 'link')} to places outside the file")]


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


def _tracked_identity(z: zipfile.ZipFile, name: str, budget: list[int]) -> tuple[set[str], bool]:
    """Who made the tracked changes in one part, and whether they carry dates.
    Read in chunks; what the cleaning tool writes in their place doesn't count."""
    authors: set[str] = set()
    dated = False
    carry = b""
    with z.open(name) as fh:
        while chunk := fh.read(CHUNK):
            budget[0] -= len(chunk)
            if budget[0] < 0:
                break
            data = carry + chunk
            last = 0
            for tag in TRACKED_TAG.finditer(data):
                last = tag.end()
                for attr in TRACKED_ATTR.finditer(tag.group(0)):
                    value = attr.group(5)[1:-1].decode("utf-8", "replace")
                    if attr.group(3) == b"author":
                        if value.strip() and value != TRACKED_PLACEHOLDER:
                            authors.add(clean_detail(value))
                    else:
                        dated = True
            carry = data[max(last, len(data) - TRACKED_CARRY) :]
    return authors, dated


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
        authors, dated = set(), False
        for n in parts:
            more, has_date = _tracked_identity(z, n, budget)
            authors |= more
            dated |= has_date
        out += [_finding("author", f"Tracked changes by {a}") for a in sorted(authors)[:8]]
        if dated:
            out.append(_finding("dates", "Tracked change dates"))
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
    # Telemetry (GPS tracks, sensor readings) travels in data streams, not tags.
    if any(s.get("codec_type") == "data" for s in info.get("streams") or []):
        out.append(_finding("hiddenContent", "Data tracks"))
    return _tidy(out)


__all__ = [
    "IDENTIFYING_KINDS",
    "clean_detail",
    "dct_image_bytes",
    "findings",
    "image_bytes_findings",
    "image_findings",
    "markup_findings",
    "media_findings",
    "ooxml_findings",
    "parse_xml",
    "pdf_findings",
    "read_part",
]
