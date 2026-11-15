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
from converter_sdk.convert import ConversionAdapter, source_format
from converter_sdk.errors import AdapterError
from converter_sdk.images import encode, open_image, pixels, read_frames
from converter_sdk.pdf import save_pdf
from converter_sdk.protocol import ValidationIssue

MB = 1024 * 1024
MAX_ENTRIES = 20_000
MAX_OFFICE_BYTES = 8 * 1024 * 1024 * 1024
MAX_REWRITE = 64 * MB
"""Largest XML part read whole to be rewritten; anything larger is copied as it is."""
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
    from lxml import etree  # noqa: PLC0415

    root = reveal.parse_xml(data)
    ns = root.tag.partition("}")[0] + "}" if root.tag.startswith("{") else ""
    for el in [e for e in root if _local(e.tag) not in APP_KEEP]:
        root.remove(el)
    app = etree.Element(f"{ns}Application")
    app.text = "Microsoft Office"
    root.insert(0, app)
    return _xml_bytes(root)


def _clean_content_types(data: bytes, dropped: set[str]) -> bytes:
    root = reveal.parse_xml(data)
    for el in list(root):
        if _local(el.tag) == "Override" and (el.get("PartName") or "").lstrip("/") in dropped:
            root.remove(el)
    return _xml_bytes(root)


def _clean_rels(name: str, data: bytes, dropped: set[str]) -> bytes:
    root = reveal.parse_xml(data)
    base = posixpath.dirname(posixpath.dirname(name))
    for el in list(root):
        target = el.get("Target") or ""
        if _local(el.tag) != "Relationship" or el.get("TargetMode") == "External":
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


def clean_ooxml(ctx: JobContext, path: Path, out: Path, fmt: str, params: dict[str, Any]) -> None:
    try:
        with zipfile.ZipFile(path) as zin:
            infos = zin.infolist()
            if len(infos) > MAX_ENTRIES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "entries"})
            if sum(i.file_size for i in infos) > MAX_OFFICE_BYTES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"reason": "size"})
            drop = _parts_to_drop([i.filename for i in infos], fmt, params)
            strip_anchors = fmt == "docx" and bool(params.get("removeComments", True))
            with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as zout:
                for info in infos:
                    ctx.check_cancelled()
                    name = info.filename
                    if name in drop or name.endswith("/"):
                        continue
                    zi = zipfile.ZipInfo(name, info.date_time)
                    zi.compress_type = zipfile.ZIP_DEFLATED
                    zi.external_attr = info.external_attr
                    new = _rewrite(zin, info, drop, strip_anchors)
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


def _rewrite(
    zin: zipfile.ZipFile, info: zipfile.ZipInfo, drop: set[str], strip_anchors: bool
) -> bytes | None:
    """The new content of a part that changes, or None to copy it as it is."""
    name = info.filename
    if name == "docProps/core.xml":
        return _clean_core(_read(zin, info))
    if name == "docProps/app.xml":
        return _clean_app(_read(zin, info))
    if name == "[Content_Types].xml" and drop:
        return _clean_content_types(_read(zin, info), drop)
    if name.endswith(".rels") and drop:
        return _clean_rels(name, _read(zin, info), drop)
    if strip_anchors and re.fullmatch(r"word/[^/]+\.xml", name) and info.file_size <= MAX_REWRITE:
        data = _read(zin, info)
        cleaned = COMMENT_ANCHORS.sub(b"", data)
        return cleaned if cleaned != data else None
    return None


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
                            z.namelist()
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
