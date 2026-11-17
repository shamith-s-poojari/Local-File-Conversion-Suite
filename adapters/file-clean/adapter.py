"""file-clean: remove hidden details from pictures, PDFs and Office files:
where a photo was taken, which camera, who wrote a document, when, and
the comments left inside. Each file keeps its format (ADR 0004).

Pictures are cleaned without re-encoding wherever the format allows it, so
the pixels stay exactly as they were. Every output is read again and the job
fails rather than hand back a file that still shows a place, a device, an
author or a date."""

from __future__ import annotations

import io
import posixpath
import re
import zipfile
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    reveal,
    run_main,
)
from converter_sdk.active import is_web_url, neutralize_relationship_ids
from converter_sdk.convert import ConversionAdapter, source_format
from converter_sdk.errors import AdapterError
from converter_sdk.images import encode, open_image, pixels, read_frames
from converter_sdk.limits import MB, check_pixels
from converter_sdk.markup import check_zip
from converter_sdk.pdf import save_pdf
from converter_sdk.protocol import ValidationIssue

MAX_ENTRIES = 20_000
MAX_OFFICE_BYTES = 8 * 1024 * 1024 * 1024
MAX_REWRITE = 64 * MB
"""Largest XML part read whole to be rewritten; a larger one that has to change
fails the job (``E_LIMIT_EXCEEDED``) rather than go out as it was."""
CHUNK = 1 << 20
IMAGES = {"jpg", "png", "webp", "tiff", "avif"}
OFFICE = {"docx", "xlsx", "pptx"}
RIGHTS = reveal.IDENTIFYING_KINDS


def corrupt() -> AdapterError:
    return AdapterError("E_INPUT_CORRUPT")


# ------------------------------------------------------------------- JPEG

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


def clean_jpeg_bytes(data: bytes) -> bytes:
    """``data`` without EXIF, XMP, IPTC, thumbnails or comments, and without
    anything after the end of the image. The compressed picture is copied
    byte for byte."""
    n = len(data)
    if data[:2] != b"\xff\xd8":
        raise corrupt()
    out = bytearray(b"\xff\xd8")
    pos, in_scan = 2, False
    while pos < n:
        if in_scan:
            m = _SCAN_MARKER.search(data, pos)
            if m is None:
                raise corrupt()
            out += data[pos : m.start()]
            pos, in_scan = m.start(), False
        if data[pos] != 0xFF:
            raise corrupt()
        while pos + 1 < n and data[pos + 1] == 0xFF:
            pos += 1  # fill bytes
        if pos + 1 >= n:
            raise corrupt()
        marker = data[pos + 1]
        if marker == 0xD9:
            out += b"\xff\xd9"
            return bytes(out)
        if marker in (0x01, 0xD8) or 0xD0 <= marker <= 0xD7:
            out += data[pos : pos + 2]
            pos += 2
            continue
        if pos + 4 > n:
            raise corrupt()
        length = int.from_bytes(data[pos + 2 : pos + 4], "big")
        end = pos + 2 + length
        if length < 2 or end > n:
            raise corrupt()
        if _keep_segment(marker, data[pos + 4 : end]):
            out += data[pos:end]
        pos = end
        in_scan = marker == 0xDA
    raise corrupt()


def clean_jpeg(ctx: JobContext, path: Path) -> tuple[bytes, bool]:
    """Returns the cleaned file and whether it had to be re-encoded (a picture
    stored sideways with an EXIF orientation is turned upright first, because
    once the orientation is gone viewers would show it the wrong way up)."""
    with open_image(path) as img:
        orientation = img.getexif().get(0x0112, 1)
    if orientation not in (None, 1):
        f = read_frames(ctx, path, first_only=True)
        data = encode(f, "jpg", quality=95, keep_exif=False)
        return clean_jpeg_bytes(data), True
    return clean_jpeg_bytes(path.read_bytes()), False


# -------------------------------------------------------------------- PNG

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
PNG_DROP = {b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"tIME"}


def clean_png_bytes(data: bytes) -> bytes:
    if data[:8] != PNG_SIGNATURE:
        raise corrupt()
    out = bytearray(PNG_SIGNATURE)
    pos, n = 8, len(data)
    while pos + 12 <= n:
        size = int.from_bytes(data[pos : pos + 4], "big")
        kind = data[pos + 4 : pos + 8]
        end = pos + 12 + size
        if end > n:
            raise corrupt()
        if kind not in PNG_DROP:
            out += data[pos:end]
        pos = end
        if kind == b"IEND":
            return bytes(out)
    raise corrupt()


# ------------------------------------------------------------------- WebP

WEBP_DROP = {b"EXIF", b"XMP "}
VP8X_METADATA_FLAGS = 0x08 | 0x04


def clean_webp_bytes(data: bytes) -> bytes:
    if data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        raise corrupt()
    end = 8 + int.from_bytes(data[4:8], "little")
    if end > len(data) or end < 12:
        raise corrupt()
    chunks = bytearray()
    pos = 12
    while pos + 8 <= end:
        fourcc = data[pos : pos + 4]
        size = int.from_bytes(data[pos + 4 : pos + 8], "little")
        body_end = pos + 8 + size
        if body_end > end:
            raise corrupt()
        if fourcc not in WEBP_DROP:
            body = bytearray(data[pos + 8 : body_end])
            if fourcc == b"VP8X" and body:
                body[0] &= ~VP8X_METADATA_FLAGS & 0xFF
            chunks += fourcc + size.to_bytes(4, "little") + body + (b"\x00" if size & 1 else b"")
        pos = body_end + (size & 1)
    return b"RIFF" + (4 + len(chunks)).to_bytes(4, "little") + b"WEBP" + bytes(chunks)


# ------------------------------------------------------------ TIFF and AVIF


def reencode(ctx: JobContext, path: Path, fmt: str) -> bytes:
    """A copy written by Pillow with no EXIF, XMP or IPTC."""
    if fmt == "avif":
        return encode(read_frames(ctx, path, first_only=True), "avif", quality=95, keep_exif=False)
    f = read_frames(ctx, path)
    if len(f.frames) == 1:
        return encode(f, "tiff", keep_exif=False)
    frames = f.frames
    buf = io.BytesIO()
    save: dict[str, Any] = {"compression": "tiff_lzw", "save_all": True,
                            "append_images": frames[1:]}  # fmt: skip
    if f.icc:
        save["icc_profile"] = f.icc
    try:
        frames[0].save(buf, "TIFF", **save)
    except (OSError, ValueError) as e:
        raise AdapterError(
            "E_INTERNAL", {"reason": "encode", "exception": type(e).__name__}
        ) from None
    return buf.getvalue()


# -------------------------------------------------------------------- PDF


def _clean_pdf_picture(obj: Any) -> None:
    """A JPEG in a PDF is the camera's file as it was: EXIF, XMP and all. The
    compressed data is kept byte for byte, so the picture doesn't change."""
    from pikepdf import Name  # noqa: PLC0415

    data = reveal.dct_image_bytes(obj)
    if data is None:
        return
    cleaned = clean_jpeg_bytes(data)
    if cleaned == data and obj.get("/Filter") == Name.DCTDecode:
        return
    # A colour-transform hint in the parameters belongs to the JPEG filter and stays.
    parms = obj.get("/DecodeParms") if obj.get("/Filter") == Name.DCTDecode else None
    obj.write(cleaned, filter=Name.DCTDecode, decode_parms=parms)


def clean_pdf(ctx: JobContext, path: Path, out: Path, params: dict[str, Any]) -> int:
    import pikepdf  # noqa: PLC0415
    from pikepdf import Name  # noqa: PLC0415

    pdf = ctx.open_pdf(path)
    try:
        if "/Info" in pdf.trailer:
            del pdf.trailer["/Info"]
        for obj in pdf.objects:
            ctx.check_cancelled()
            if not isinstance(obj, pikepdf.Dictionary | pikepdf.Stream):
                continue
            for key in (Name.Metadata, Name.PieceInfo, Name.Thumb):
                if key in obj:
                    del obj[key]
            if isinstance(obj, pikepdf.Stream):
                _clean_pdf_picture(obj)
        if params.get("removeComments", True):
            for page in pdf.pages:
                ctx.check_cancelled()
                annots = page.obj.get(Name.Annots)
                if annots is None:
                    continue
                kept = [
                    a
                    for a in annots
                    if isinstance(a, pikepdf.Dictionary)
                    and a.get(Name.Subtype) in (Name.Link, Name.Widget)
                ]
                if kept:
                    page.obj[Name.Annots] = pikepdf.Array(kept)
                else:
                    del page.obj[Name.Annots]
        if params.get("removeHiddenLayers", False):
            try:
                from converter_sdk.pdf import remove_hidden_layers  # noqa: PLC0415
            except ImportError:
                raise AdapterError("E_INTERNAL", {"reason": "hiddenLayersUnavailable"}) from None
            remove_hidden_layers(pdf)
        from converter_sdk.pdf import is_signed  # noqa: PLC0415

        if is_signed(pdf):
            ctx.warn("SIGNATURE_INVALIDATED")
        return save_pdf(ctx, pdf, out)
    finally:
        pdf.close()


# ------------------------------------------------------------------ OOXML

COMMENT_PARTS = {
    "docx": re.compile(
        r"word/(?:comments|commentsExtended|commentsIds|commentsExtensible|people)\.xml"
    ),
    "xlsx": re.compile(r"xl/(?:comments\d*\.xml|threadedComments/[^/]+\.xml|persons/[^/]+\.xml)"),
    "pptx": re.compile(r"ppt/(?:comments/[^/]+\.xml|commentAuthors\.xml|authors\.xml)"),
}
CORE_DROP = {"creator", "lastModifiedBy", "revision", "lastPrinted", "created", "modified"}
APP_KEEP = {
    "DocSecurity", "ScaleCrop", "LinksUpToDate", "SharedDoc", "HyperlinksChanged", "Pages",
    "Words", "Characters", "CharactersWithSpaces", "Lines", "Paragraphs", "Slides", "Notes",
    "HiddenSlides", "MMClips",
}  # fmt: skip
COMMENT_ANCHORS = re.compile(rb"<w:(?:commentRangeStart|commentRangeEnd|commentReference)\b[^>]*/>")


def _xml_bytes(root: Any) -> bytes:
    from lxml import etree  # noqa: PLC0415

    return etree.tostring(root, xml_declaration=True, encoding="UTF-8", standalone=True)


def _local(tag: Any) -> str:
    return tag.rpartition("}")[2] if isinstance(tag, str) else ""


def _read(zin: zipfile.ZipFile, info: zipfile.ZipInfo, limit: int = MAX_REWRITE) -> bytes:
    if info.file_size > limit:
        raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "partSize"})
    with zin.open(info) as fh:
        return fh.read()


def _clean_core(data: bytes) -> bytes:
    root = reveal.parse_xml(data)
    for el in [e for e in root if _local(e.tag) in CORE_DROP]:
        root.remove(el)
    return _xml_bytes(root)


def _clean_app(data: bytes) -> bytes:
    """Only the counts (pages, words) stay. The program that wrote the file is
    left out, not replaced: naming one would be making it up."""
    root = reveal.parse_xml(data)
    for el in [e for e in root if _local(e.tag) not in APP_KEEP]:
        root.remove(el)
    return _xml_bytes(root)


def _clean_content_types(data: bytes, dropped: set[str]) -> bytes:
    root = reveal.parse_xml(data)
    for el in list(root):
        if _local(el.tag) == "Override" and (el.get("PartName") or "").lstrip("/") in dropped:
            root.remove(el)
    return _xml_bytes(root)


def _clean_rels(name: str, data: bytes, dropped: set[str], outside: set[str]) -> bytes:
    """Delete the relationships to dropped parts and the ids in ``outside``."""
    root = reveal.parse_xml(data)
    base = posixpath.dirname(posixpath.dirname(name))
    for el in list(root):
        if _local(el.tag) != "Relationship":
            continue
        if el.get("Id") in outside:
            root.remove(el)
            continue
        target = el.get("Target") or ""
        if el.get("TargetMode") == "External":
            continue
        resolved = posixpath.normpath(
            target.lstrip("/") if target.startswith("/") else posixpath.join(base, target)
        )
        if resolved in dropped:
            root.remove(el)
    return _xml_bytes(root)


def _parts_to_drop(names: list[str], fmt: str, params: dict[str, Any]) -> set[str]:
    drop = {n for n in names if n == "docProps/custom.xml" or n.startswith("docProps/thumbnail.")}
    if params.get("removeComments", True):
        drop |= {n for n in names if COMMENT_PARTS[fmt].fullmatch(n)}
    # A dropped part's own relationships go with it.
    for n in list(drop):
        d, _, base = n.rpartition("/")
        drop.add(f"{d}/_rels/{base}.rels" if d else f"_rels/{base}.rels")
    return drop


def _source_of(rels: str) -> str:
    """The part a ``.rels`` file belongs to (``""`` for the package itself)."""
    directory, _, base = rels.rpartition("/")
    directory = directory.removesuffix("_rels").rstrip("/")
    name = base.removesuffix(".rels")
    return f"{directory}/{name}" if directory and name else name


class _Plan:
    """What to change in one Office package, worked out before any part is written."""

    def __init__(self, zin: zipfile.ZipFile, infos: list[zipfile.ZipInfo], fmt: str,
                 params: dict[str, Any]) -> None:  # fmt: skip
        self.fmt = fmt
        self.drop = _parts_to_drop([i.filename for i in infos], fmt, params)
        self.strip_anchors = fmt == "docx" and bool(params.get("removeComments", True))
        # External relationships other than plain web links: the template, a
        # linked picture or file. They name the author's folders and servers.
        self.outside_by_rels: dict[str, set[str]] = {}
        self.outside_by_source: dict[str, set[str]] = {}
        for info in infos:
            name = info.filename
            if (
                not name.endswith(".rels")
                or name in self.drop
                or name.lower().startswith("xl/externallinks/")
            ):
                continue
            ids = set()
            for rel in reveal.parse_xml(_read(zin, info)):
                if _local(rel.tag) != "Relationship":
                    continue
                if (rel.get("TargetMode") or "").lower() != "external":
                    continue
                hyperlink = (rel.get("Type") or "").lower().endswith("/hyperlink")
                if not (hyperlink and is_web_url(rel.get("Target") or "")):
                    ids.add(rel.get("Id") or "")
            if ids:
                self.outside_by_rels[name] = ids
                if source := _source_of(name):
                    self.outside_by_source[source] = ids
        self.outside = sum(len(v) for v in self.outside_by_rels.values())


def clean_ooxml(ctx: JobContext, path: Path, out: Path, fmt: str, params: dict[str, Any]) -> None:
    try:
        with zipfile.ZipFile(path) as zin:
            infos = check_zip(zin, member_max=MAX_OFFICE_BYTES, total_max=MAX_OFFICE_BYTES)
            if len(infos) > MAX_ENTRIES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "entries"})
            plan = _Plan(zin, infos, fmt, params)
            with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as zout:
                for info in infos:
                    ctx.check_cancelled()
                    name = info.filename
                    if name in plan.drop or name.endswith("/"):
                        continue
                    # Every part gets the same date: the ZIP records when each was saved.
                    zi = zipfile.ZipInfo(name, reveal.FIXED_ZIP_DATE)
                    zi.compress_type = zipfile.ZIP_DEFLATED
                    zi.external_attr = info.external_attr
                    new = _rewrite(zin, info, plan)
                    if new is not None:
                        zout.writestr(zi, new)
                        continue
                    with (
                        zin.open(info) as src,
                        zout.open(zi, "w", force_zip64=info.file_size > zipfile.ZIP64_LIMIT) as dst,
                    ):
                        while chunk := src.read(CHUNK):
                            dst.write(chunk)
    except (zipfile.BadZipFile, EOFError, NotImplementedError, RuntimeError):
        out.unlink(missing_ok=True)
        raise corrupt() from None


def _blank_tracked(data: bytes) -> bytes:
    """Tracked changes keep their text but not who made them or when."""

    def attribute(m: re.Match[bytes]) -> bytes:
        if m.group(3) == b"author":
            return m.group(1) + m.group(2) + b"author" + m.group(4) + b'"Author"'
        return b""

    return reveal.TRACKED_TAG.sub(
        lambda tag: reveal.TRACKED_ATTR.sub(attribute, tag.group(0)), data
    )


def _clean_picture(data: bytes) -> bytes | None:
    """A picture from ``media/`` without its metadata, or None if it isn't a
    kind of picture that carries any. Identified by content, not by name."""
    if data[:2] == b"\xff\xd8":
        return clean_jpeg_bytes(data)
    if data[:8] == PNG_SIGNATURE:
        return clean_png_bytes(data)
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return clean_webp_bytes(data)
    if data[:4] in (b"II*\x00", b"MM\x00*"):
        return _clean_tiff_bytes(data)
    return None


def _clean_tiff_bytes(data: bytes) -> bytes:
    from PIL import Image, ImageSequence  # noqa: PLC0415

    try:
        with Image.open(io.BytesIO(data)) as img:
            check_pixels(img.width, img.height, int(getattr(img, "n_frames", 1)))
            frames = [f.copy() for f in ImageSequence.Iterator(img)]
            icc = img.info.get("icc_profile")
        buf = io.BytesIO()
        save: dict[str, Any] = {"compression": "tiff_lzw", "save_all": len(frames) > 1,
                                "append_images": frames[1:]}  # fmt: skip
        if icc:
            save["icc_profile"] = icc
        frames[0].save(buf, "TIFF", **save)
    except (OSError, ValueError, SyntaxError, EOFError, Image.DecompressionBombError):
        raise corrupt() from None
    return buf.getvalue()


def _rewrite(zin: zipfile.ZipFile, info: zipfile.ZipInfo, plan: _Plan) -> bytes | None:
    """The new content of a part that changes, or None to copy it as it is."""
    name = info.filename
    if name == "docProps/core.xml":
        return _clean_core(_read(zin, info))
    if name == "docProps/app.xml":
        return _clean_app(_read(zin, info))
    if name == "[Content_Types].xml" and plan.drop:
        return _clean_content_types(_read(zin, info), plan.drop)
    if name.endswith(".rels"):
        outside = plan.outside_by_rels.get(name, set())
        if plan.drop or outside:
            return _clean_rels(name, _read(zin, info), plan.drop, outside)
        return None
    if reveal.MEDIA_PART.fullmatch(name):
        if info.file_size > reveal.MAX_MEDIA_BYTES:
            raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "pictureSize"})
        data = _read(zin, info, reveal.MAX_MEDIA_BYTES)
        cleaned = _clean_picture(data)
        return None if cleaned is None or cleaned == data else cleaned
    is_word = plan.fmt == "docx" and re.fullmatch(r"word/[^/]+\.xml", name) is not None
    outside = plan.outside_by_source.get(name, set())
    if not (is_word or outside):
        return None
    data = _read(zin, info)
    changed = data
    if is_word:
        if plan.strip_anchors:
            changed = COMMENT_ANCHORS.sub(b"", changed)
        changed = _blank_tracked(changed)
    if outside:
        root = reveal.parse_xml(changed)
        if neutralize_relationship_ids(root, outside):
            changed = _xml_bytes(root)
    return changed if changed != data else None


# ---------------------------------------------------------------- adapter


class FileCleanAdapter(Adapter):
    adapter_id: ClassVar[str] = "file-clean"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        report = ValidationReport()
        sizes, pix = [], []
        for p in ctx.inputs:
            ctx.check_cancelled()
            fmt = source_format(p)
            sizes.append(p.stat().st_size)
            try:
                if fmt in IMAGES:
                    pix.append(pixels(p))
                elif fmt == "pdf":
                    ctx.open_pdf(p).close()
                elif fmt in OFFICE:
                    try:
                        with zipfile.ZipFile(p) as z:
                            check_zip(z, member_max=MAX_OFFICE_BYTES, total_max=MAX_OFFICE_BYTES)
                    except (zipfile.BadZipFile, OSError):
                        raise corrupt() from None
            except AdapterError as e:
                report.errors.append(
                    ValidationIssue(code=e.code, input=p.relative_to(ctx.job_dir).as_posix())
                )
        report.metadata = {"bytes": sizes, "pixels": pix}
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        sizes = metadata.get("bytes") or [0]
        pix = metadata.get("pixels") or [0]
        return ResourceEstimate(
            ram_bytes=round(128 * MB + max(sizes) * 6 + max(pix) * 16),
            disk_bytes=round(sum(sizes) * 2) + MB,
            cost_units=max(1, len(sizes)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        outputs: list[OutputFile] = []
        flags = {"removed": False, "reencoded": False, "tracked": False}
        total = len(ctx.inputs)
        ctx.stage("clean", 1, 1)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            fmt = source_format(path)
            before = set(_keys(reveal.findings(ctx, path, include_active=False)))
            out = ctx.output_path(f"{ConversionAdapter.title(params, i)}-clean.{fmt}")
            pages = self.clean(ctx, path, out, fmt, params, flags)
            after_list = reveal.findings(ctx, out, include_active=False)
            if any(f.kind in RIGHTS for f in after_list):
                out.unlink(missing_ok=True)
                raise AdapterError("E_INTERNAL", {"reason": "detailsRemain"})
            after = set(_keys(after_list))
            flags["removed"] |= bool(before - after)
            flags["tracked"] |= any(f.kind == "trackedChanges" for f in after_list)
            outputs.append(OutputFile(out, pages=pages, source=i))
            ctx.progress("clean", i + 1, total)
        if flags["removed"]:
            ctx.warn("DETAILS_REMOVED")
        if flags["tracked"]:
            ctx.warn("TRACKED_CHANGES_KEPT")
        if flags["reencoded"]:
            ctx.warn("REENCODED")
        return outputs

    def clean(
        self,
        ctx: JobContext,
        path: Path,
        out: Path,
        fmt: str,
        params: dict[str, Any],
        flags: dict[str, bool],
    ) -> int | None:
        if fmt == "pdf":
            return clean_pdf(ctx, path, out, params)
        if fmt in OFFICE:
            clean_ooxml(ctx, path, out, fmt, params)
            return None
        if fmt == "jpg":
            data, again = clean_jpeg(ctx, path)
            flags["reencoded"] |= again
        elif fmt == "png":
            data = clean_png_bytes(path.read_bytes())
        elif fmt == "webp":
            data = clean_webp_bytes(path.read_bytes())
        elif fmt in ("tiff", "avif"):
            data = reencode(ctx, path, fmt)
            flags["reencoded"] = True
        else:
            raise AdapterError("E_INTERNAL", {"reason": "unsupportedFormat"}, retryable=False)
        out.write_bytes(data)
        return None


def _keys(found: list[Any]) -> list[tuple[str, str | None]]:
    return [(f.kind, f.detail) for f in found]


if __name__ == "__main__":
    run_main(FileCleanAdapter)
