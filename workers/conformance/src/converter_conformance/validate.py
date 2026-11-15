"""Is an output a valid file of its format? (ADR 0003)

Each check opens the file with an independent reader, the way a user's
program would. Returns the page count for PDFs and ``None`` otherwise;
raises ``AssertionError`` with a short reason when the file is invalid.
"""

from __future__ import annotations

import csv
import hashlib
import io
import json
import subprocess
import zipfile
from pathlib import Path

import yaml

from . import files

RASTER = {
    "png": "PNG",
    "jpg": "JPEG",
    "webp": "WEBP",
    "gif": "GIF",
    "bmp": "BMP",
    "tiff": "TIFF",
    "avif": "AVIF",
    "ico": "ICO",
}
AUDIO = set(files.AUDIO_FORMATS)
VIDEO = set(files.VIDEO_FORMATS)
ZIP_MIMETYPES = {
    "odt": "application/vnd.oasis.opendocument.text",
    "ods": "application/vnd.oasis.opendocument.spreadsheet",
    "odp": "application/vnd.oasis.opendocument.presentation",
    "epub": "application/epub+zip",
}
OOXML_MAIN = {
    "docx": "word/document.xml",
    "xlsx": "xl/workbook.xml",
    "pptx": "ppt/presentation.xml",
}
CFB = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"


def check(path: Path, fmt: str, password: str = "") -> int | None:
    if fmt == "pdf":
        import pikepdf  # noqa: PLC0415

        with pikepdf.open(path, password=password) as pdf:
            assert pdf.check_pdf_syntax() == [], "pdf fails check"
            return len(pdf.pages)
    if fmt in RASTER:
        from PIL import Image  # noqa: PLC0415

        if fmt == "avif" or fmt == "heic":
            pass
        with Image.open(path) as img:
            assert img.format == RASTER[fmt], f"{img.format} is not {fmt}"
            img.load()
        return None
    if fmt == "svg":
        text = path.read_text("utf-8")
        assert "<svg" in text[:2000], "no svg root"
        return None
    if fmt in AUDIO or fmt in VIDEO:
        info = probe(path)
        streams = info.get("streams", [])
        kinds = {s.get("codec_type") for s in streams}
        assert "audio" in kinds or "video" in kinds, "no streams"
        if fmt in AUDIO:
            assert "video" not in {
                s.get("codec_type")
                for s in streams
                if not s.get("disposition", {}).get("attached_pic")
            }, "audio file has video"
        assert float(info.get("format", {}).get("duration", 0) or 0) > 0, "zero duration"
        return None
    if fmt in OOXML_MAIN:
        with zipfile.ZipFile(path) as z:
            assert z.testzip() is None, "bad zip member"
            assert OOXML_MAIN[fmt] in z.namelist(), f"no {OOXML_MAIN[fmt]}"
        return None
    if fmt in ZIP_MIMETYPES:
        with zipfile.ZipFile(path) as z:
            assert z.testzip() is None, "bad zip member"
            assert z.read("mimetype").decode().strip() == ZIP_MIMETYPES[fmt], "wrong mimetype"
        return None
    if fmt in ("doc", "xls", "ppt"):
        assert path.read_bytes()[:8] == CFB, "not a compound file"
        return None
    text = path.read_bytes().decode("utf-8-sig")
    if fmt == "json":
        json.loads(text)
    elif fmt == "yaml":
        yaml.safe_load(text)
    elif fmt == "csv":
        rows = list(csv.reader(io.StringIO(text)))
        assert rows, "empty csv"
    elif fmt == "rtf":
        assert text.lstrip().startswith("{\\rtf"), "not rtf"
    elif fmt == "html":
        assert "<" in text, "no markup"
    return None


def probe(path: Path) -> dict:
    exe = files.engine_env().get("LFCS_BIN_FFPROBE")
    assert exe, "ffprobe missing"
    out = subprocess.run(  # noqa: S603 - fixed arguments
        [exe, "-v", "error", "-print_format", "json", "-show_format", "-show_streams", str(path)],
        capture_output=True,
        check=True,
    )
    return json.loads(out.stdout)


def fingerprint(path: Path, fmt: str, password: str = "") -> object:
    """What must match between two runs of the same job."""
    if fmt == "pdf":
        from . import corpus  # noqa: PLC0415

        return corpus.page_texts(path, password)
    if fmt in RASTER:
        from PIL import Image  # noqa: PLC0415

        with Image.open(path) as img:
            return hashlib.sha256(img.convert("RGBA").tobytes()).hexdigest()
    if fmt in AUDIO or fmt in VIDEO:
        info = probe(path)
        return sorted(
            (s.get("codec_type"), s.get("codec_name"), round(float(s.get("duration", 0) or 0), 1))
            for s in info.get("streams", [])
        )
    if fmt in OOXML_MAIN or fmt in ZIP_MIMETYPES:
        with zipfile.ZipFile(path) as z:
            return sorted(z.namelist())
    return hashlib.sha256(path.read_bytes()).hexdigest()
