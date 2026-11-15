"""Generated non-PDF test files (ADR 0003): images, audio, video, documents
and data. Like ``corpus``, everything is built on demand, so the repository
holds no binary fixtures.

Names are ``<kind>-<format>`` (``image-png``), with ``corrupt-<format>`` and
``truncated-<format>`` variants. Audio and video need FFmpeg; tests that use
them are skipped when it's missing (see ``ffmpeg()``). Legacy and
OpenDocument office files are saved by LibreOffice, so they need it.
"""

from __future__ import annotations

import io
import json
import math
import os
import shutil
import struct
import subprocess
import tempfile
import wave
import zipfile
from collections.abc import Callable
from pathlib import Path

from PIL import Image, ImageDraw

# ------------------------------------------------------------------ engines


def ffmpeg() -> str | None:
    """FFmpeg for building media fixtures and for adapters under test: from
    ``LFCS_BIN_FFMPEG``, else ``PATH`` and the usual install folders."""
    found = os.environ.get("LFCS_BIN_FFMPEG") or shutil.which("ffmpeg")
    for candidate in ("/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"):
        if not found and Path(candidate).exists():
            found = candidate
    return found


def soffice() -> str | None:
    found = os.environ.get("LFCS_BIN_SOFFICE") or shutil.which("soffice")
    program_files = [os.environ.get(v) for v in ("ProgramFiles", "ProgramFiles(x86)")]
    for candidate in (
        "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        "/usr/bin/soffice",
        "/usr/lib/libreoffice/program/soffice",
        *(str(Path(p, "LibreOffice", "program", "soffice.com")) for p in program_files if p),
    ):
        if not found and Path(candidate).exists():
            found = candidate
    return found


def engine_env() -> dict[str, str]:
    """``LFCS_BIN_*`` for every engine found, as the core would set them."""
    env = {}
    if f := ffmpeg():
        env["LFCS_BIN_FFMPEG"] = os.path.realpath(f)
        real = Path(os.path.realpath(f))
        probe = real.with_name("ffprobe" + real.suffix)  # ffprobe.exe beside ffmpeg.exe
        if probe.exists():
            env["LFCS_BIN_FFPROBE"] = str(probe)
    if s := soffice():
        env["LFCS_BIN_SOFFICE"] = os.path.realpath(s)
    return env


# ------------------------------------------------------------------- images


def _picture(size: tuple[int, int] = (320, 200), alpha: bool = False) -> Image.Image:
    """A gradient with a red square and a label: something every encoder keeps."""
    w, h = size
    img = Image.new("RGBA" if alpha else "RGB", size, (255, 255, 255, 0) if alpha else "white")
    draw = ImageDraw.Draw(img)
    for x in range(w):
        shade = int(255 * x / max(1, w - 1))
        draw.line([(x, 0), (x, h // 3)], fill=(shade, 128, 255 - shade, 255))
    draw.rectangle([w // 4, h // 2, w // 2, h - 10], fill=(220, 20, 20, 255))
    draw.text((10, h // 3 + 8), "Fixture", fill=(0, 0, 0, 255))
    return img


def image_file(path: Path, fmt: str) -> Path:
    if fmt == "svg":
        path.write_text(
            '<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100" '
            'viewBox="0 0 160 100">'
            '<rect x="10" y="10" width="60" height="60" fill="#d01414"/>'
            '<circle cx="115" cy="45" r="30" fill="#1450d0"/>'
            '<text x="10" y="92" font-size="14" font-family="sans-serif">Fixture</text></svg>',
            "utf-8",
        )
        return path
    if fmt == "heic":
        import pillow_heif  # noqa: PLC0415

        pillow_heif.register_heif_opener()
        _picture().save(path, "HEIF", quality=90)
        return path
    if fmt == "gif":
        frames = []
        for i in range(4):
            f = _picture((160, 100)).convert("P", palette=Image.Palette.ADAPTIVE)
            ImageDraw.Draw(f).rectangle([i * 30, 0, i * 30 + 20, 20], fill=0)
            frames.append(f)
        frames[0].save(path, "GIF", save_all=True, append_images=frames[1:], duration=120, loop=0)
        return path
    if fmt == "jpg":
        # Stored sideways with EXIF orientation 6: viewers rotate it upright.
        img = _picture((200, 320)).transpose(Image.Transpose.ROTATE_90)
        exif = Image.Exif()
        exif[0x0112] = 6
        exif[0x010F] = "Fixture Camera"
        img.save(path, "JPEG", quality=92, exif=exif.tobytes())
        return path
    pil = {
        "png": "PNG",
        "webp": "WEBP",
        "bmp": "BMP",
        "tiff": "TIFF",
        "avif": "AVIF",
        "ico": "ICO",
    }[fmt]
    alpha = fmt in ("png", "webp", "avif", "ico")
    img = _picture((256, 256) if fmt == "ico" else (320, 200), alpha=alpha)
    img.save(path, pil)
    return path


def big_png(path: Path, count: int = 1) -> Path:
    """A large, incompressible image: slow enough to cancel mid-way."""
    data = os.urandom(3000 * 3000 * 3)
    Image.frombytes("RGB", (3000, 3000), data).save(path, "PNG", compress_level=9)
    return path


# -------------------------------------------------------------------- audio


def wav_tone(path: Path, seconds: float = 2.0, rate: int = 44100, channels: int = 2) -> Path:
    """A 440 Hz tone (left) and 660 Hz (right), 16-bit PCM, made without FFmpeg."""
    frames = int(seconds * rate)
    buf = bytearray()
    for n in range(frames):
        left = int(12000 * math.sin(2 * math.pi * 440 * n / rate))
        right = int(12000 * math.sin(2 * math.pi * 660 * n / rate))
        buf += struct.pack("<hh", left, right) if channels == 2 else struct.pack("<h", left)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(channels)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(bytes(buf))
    return path


def _ffmpeg_make(path: Path, *args: str) -> Path:
    exe = ffmpeg()
    if exe is None:
        raise FileNotFoundError("ffmpeg")
    subprocess.run(  # noqa: S603 - fixed arguments, test fixture
        [exe, "-hide_banner", "-loglevel", "error", "-y", *args, str(path)],
        check=True,
        stdin=subprocess.DEVNULL,
    )
    return path


AUDIO_ARGS = {
    "mp3": ["-c:a", "libmp3lame", "-b:a", "128k"],
    "flac": ["-c:a", "flac"],
    "ogg": ["-c:a", "libvorbis", "-q:a", "4"],
    "opus": ["-c:a", "libopus", "-b:a", "96k"],
    "m4a": ["-c:a", "aac", "-b:a", "128k", "-f", "ipod"],
    "aac": ["-c:a", "aac", "-b:a", "128k", "-f", "adts"],
    "aiff": ["-c:a", "pcm_s16be", "-f", "aiff"],
    "wav": ["-c:a", "pcm_s16le"],
}


def audio_file(path: Path, fmt: str, seconds: float = 3.0) -> Path:
    if fmt == "wav":
        return wav_tone(path, seconds)
    src = wav_tone(path.with_suffix(".src.wav"), seconds)
    try:
        return _ffmpeg_make(path, "-i", str(src), *AUDIO_ARGS[fmt])
    finally:
        src.unlink(missing_ok=True)


VIDEO_ARGS = {
    "mp4": ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"],
    "mov": ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-f", "mov"],
    "mkv": ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "libvorbis"],
    "webm": ["-c:v", "libvpx-vp9", "-b:v", "200k", "-deadline", "realtime", "-c:a", "libopus"],
    "avi": ["-c:v", "mpeg4", "-q:v", "5", "-c:a", "libmp3lame"],
}


def video_file(path: Path, fmt: str, seconds: float = 2.0, size: str = "320x240") -> Path:
    return _ffmpeg_make(
        path,
        "-f", "lavfi", "-i", f"testsrc2=size={size}:rate=25:duration={seconds}",
        "-f", "lavfi", "-i", f"sine=frequency=440:duration={seconds}",
        "-shortest",
        *VIDEO_ARGS[fmt],
    )  # fmt: skip


def silent_video(path: Path, fmt: str = "mp4", seconds: float = 2.0) -> Path:
    return _ffmpeg_make(
        path, "-f", "lavfi", "-i", f"testsrc2=size=320x240:rate=25:duration={seconds}",
        "-c:v", "libx264", "-pix_fmt", "yuv420p",
    )  # fmt: skip


# ---------------------------------------------------------------- documents


PARAGRAPHS = [
    "Quarterly report",
    "Revenue grew in every region this quarter.",
    "The second paragraph mentions Lisbon and 47.2 percent.",
]


def docx_file(path: Path) -> Path:
    import docx  # noqa: PLC0415 - python-docx, a pdf2docx dependency

    d = docx.Document()
    d.add_heading(PARAGRAPHS[0], level=1)
    for p in PARAGRAPHS[1:]:
        d.add_paragraph(p)
    table = d.add_table(rows=2, cols=2)
    for r, row in enumerate([["Region", "Sales"], ["North", "1200"]]):
        for c, text in enumerate(row):
            table.cell(r, c).text = text
    d.save(path)
    return path


def xlsx_file(path: Path) -> Path:
    import openpyxl  # noqa: PLC0415

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Sales"
    ws.append(["Region", "Q1", "Q2", "Total"])
    for i, (region, q1, q2) in enumerate([("North", 1200, 1350), ("South", 980, 1010)], start=2):
        ws.append([region, q1, q2, f"=B{i}+C{i}"])
    other = wb.create_sheet("Notes")
    other.append(["Note"])
    other.append(["Figures are in euros"])
    wb.save(path)
    return path


def text_file(path: Path, fmt: str) -> Path:
    body = {
        "txt": "\n\n".join(PARAGRAPHS) + "\n",
        "md": f"# {PARAGRAPHS[0]}\n\n{PARAGRAPHS[1]}\n\n- **Lisbon**\n- *47.2 percent*\n\n"
        "| Region | Sales |\n|---|---|\n| North | 1200 |\n",
        "html": "<!DOCTYPE html><html><head><meta charset='utf-8'>"
        "<title>Report</title></head><body>"
        f"<h1>{PARAGRAPHS[0]}</h1><p>{PARAGRAPHS[1]}</p><p>{PARAGRAPHS[2]}</p>"
        "<table><tr><th>Region</th><th>Sales</th></tr><tr><td>North</td><td>1200</td></tr></table>"
        "</body></html>",
        "csv": 'Region,Q1,Q2\nNorth,1200,1350\nSouth,980,1010\n"East, coast",700,"7,5"\n',
        "json": json.dumps(
            [
                {"Region": "North", "Q1": 1200, "Q2": 1350},
                {"Region": "South", "Q1": 980, "Q2": 1010},
            ],
            indent=2,
        ),
        "yaml": "- Region: North\n  Q1: 1200\n  Q2: 1350\n- Region: South\n  Q1: 980\n  Q2: 1010\n",
        "rtf": "{\\rtf1\\ansi{\\fonttbl\\f0\\fswiss Helvetica;}\\f0\\pard "
        + "\\par ".join(PARAGRAPHS)
        + "\\par}",
    }[fmt]
    path.write_text(body, "utf-8")
    return path


def long_csv(path: Path, rows: int = 300_000) -> Path:
    """A table big enough to take seconds, for cancel tests."""
    lines = ["id,name,city,amount,note"]
    for i in range(rows):
        lines.append(f'{i},Person {i},"Lisbon, PT",{i * 1.25:.2f},row {i} of the report')
    path.write_text("\n".join(lines) + "\n", "utf-8")
    return path


def long_markdown(path: Path, sections: int = 4000) -> Path:
    """Hundreds of pages once laid out, for cancel tests."""
    parts = []
    for i in range(sections):
        parts.append(f"## Section {i + 1}\n\n{PARAGRAPHS[1]} {PARAGRAPHS[2]} " * 3)
        parts.append("| Region | Sales |\n|---|---|\n| North | 1200 |\n| South | 980 |\n")
    path.write_text("\n\n".join(parts), "utf-8")
    return path


def epub_file(path: Path) -> Path:
    """A minimal EPUB 3: mimetype first and stored, then the container and one chapter."""
    chapter = (
        '<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml">'
        f"<head><title>{PARAGRAPHS[0]}</title></head><body><h1>{PARAGRAPHS[0]}</h1>"
        f"<p>{PARAGRAPHS[1]}</p><p>{PARAGRAPHS[2]}</p></body></html>"
    )
    opf = (
        '<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" '
        'version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">'
        '<dc:identifier id="id">fixture</dc:identifier><dc:title>Fixture</dc:title>'
        "<dc:language>en</dc:language></metadata><manifest>"
        '<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>'
        '<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>'
        '</manifest><spine><itemref idref="c1"/></spine></package>'
    )
    nav = (
        '<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" '
        'xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Nav</title></head><body>'
        '<nav epub:type="toc"><ol><li><a href="c1.xhtml">One</a></li></ol></nav></body></html>'
    )
    container = (
        '<?xml version="1.0"?><container version="1.0" '
        'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
        '<rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>'
        "</rootfiles></container>"
    )
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(
            zipfile.ZipInfo("mimetype"), "application/epub+zip", compress_type=zipfile.ZIP_STORED
        )
        z.writestr("META-INF/container.xml", container, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/content.opf", opf, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/c1.xhtml", chapter, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/nav.xhtml", nav, compress_type=zipfile.ZIP_DEFLATED)
    return path


def _soffice_make(path: Path, src: Path, filter_spec: str, infilter: str | None = None) -> Path:
    """Convert ``src`` with LibreOffice into ``path``. Fixtures only: runs
    unsandboxed, but with a throwaway profile, never the user's."""
    exe = soffice()
    if exe is None:
        raise RuntimeError("LibreOffice isn't installed")
    with tempfile.TemporaryDirectory() as tmp:
        argv = [exe, f"-env:UserInstallation={Path(tmp, 'profile').as_uri()}",
                "--headless", "--nologo", "--norestore", "--nodefault",
                "--nolockcheck"]  # fmt: skip
        if infilter:
            argv.append(f"--infilter={infilter}")
        argv += ["--convert-to", filter_spec, "--outdir", tmp, str(src)]
        env = {**os.environ, "HOME": tmp, "TMPDIR": tmp}
        subprocess.run(argv, check=True, capture_output=True, timeout=180, env=env)  # noqa: S603
        produced = next(f for f in Path(tmp).iterdir() if f.is_file() and f.suffix != src.suffix)
        shutil.move(produced, path)
    return path


def slides_file(path: Path) -> Path:
    """A two-slide PowerPoint: a PDF drawn with PyMuPDF, imported by Impress."""
    import pymupdf  # noqa: PLC0415

    pdf = path.with_suffix(".src.pdf")
    doc = pymupdf.open()
    for title, line in [(PARAGRAPHS[0], PARAGRAPHS[1]), ("Lisbon", PARAGRAPHS[2])]:
        page = doc.new_page(width=720, height=405)
        page.insert_text((60, 120), title, fontsize=36)
        page.insert_text((60, 200), line, fontsize=18)
    doc.save(pdf)
    doc.close()
    try:
        return _soffice_make(
            path, pdf, "pptx:Impress MS PowerPoint 2007 XML", infilter="impress_pdf_import"
        )
    finally:
        pdf.unlink(missing_ok=True)


def report_pdf(path: Path) -> Path:
    """A two-page report as a word processor would print it: a bold 22 pt
    heading, 11 pt paragraphs (one with a bold phrase), a ruled table, a
    second-level heading and a small picture."""
    import pymupdf  # noqa: PLC0415

    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_text((72, 100), PARAGRAPHS[0], fontsize=22, fontname="hebo")
    page.insert_text((72, 140), PARAGRAPHS[1], fontsize=11, fontname="helv")
    page.insert_text((72, 158), "Totals are", fontsize=11, fontname="helv")
    page.insert_text((128, 158), "final", fontsize=11, fontname="hebo")
    page.insert_text((158, 158), "for every region.", fontsize=11, fontname="helv")
    rows = [["Region", "Q1", "Q2"], ["North", "1200", "1350"], ["South", "980", "1010"]]
    x0, y0, cw, rh = 72, 190, 120, 22
    for r, row in enumerate(rows):
        for c, text in enumerate(row):
            rect = pymupdf.Rect(x0 + c * cw, y0 + r * rh, x0 + (c + 1) * cw, y0 + (r + 1) * rh)
            page.draw_rect(rect, color=(0, 0, 0), width=0.8)
            page.insert_text(
                (rect.x0 + 5, rect.y1 - 7), text, fontsize=10, fontname="hebo" if r == 0 else "helv"
            )
    page = doc.new_page(width=595, height=842)
    page.insert_text((72, 100), "Outlook", fontsize=16, fontname="hebo")
    page.insert_text((72, 130), PARAGRAPHS[2], fontsize=11, fontname="helv")
    picture = io.BytesIO()
    _picture((120, 80)).convert("RGB").save(picture, "PNG")
    page.insert_image(pymupdf.Rect(72, 160, 192, 240), stream=picture.getvalue())
    doc.save(path, garbage=3, deflate=True)
    doc.close()
    return path


def scan_pdf(path: Path) -> Path:
    """A page that is only a picture, like a scan with no text layer."""
    import pymupdf  # noqa: PLC0415

    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    picture = io.BytesIO()
    _picture((400, 300)).convert("RGB").save(picture, "PNG")
    page.insert_image(pymupdf.Rect(72, 72, 472, 372), stream=picture.getvalue())
    doc.save(path)
    doc.close()
    return path


OFFICE_FROM = {
    "doc": (docx_file, "docx", "doc:MS Word 97"),
    "odt": (docx_file, "docx", "odt:writer8"),
    "xls": (xlsx_file, "xlsx", "xls:MS Excel 97"),
    "ods": (xlsx_file, "xlsx", "ods:calc8"),
    "ppt": (slides_file, "pptx", "ppt:MS PowerPoint 97"),
    "odp": (slides_file, "pptx", "odp:impress8"),
}


def office_file(path: Path, fmt: str) -> Path:
    """Legacy and OpenDocument files, saved by LibreOffice from our own
    DOCX, XLSX and PPTX."""
    make, ext, filter_spec = OFFICE_FROM[fmt]
    src = make(path.with_name(f"{path.stem}-src.{ext}"))
    try:
        return _soffice_make(path, src, filter_spec)
    finally:
        src.unlink(missing_ok=True)


# ------------------------------------------------------------ broken inputs


def corrupt(path: Path, fmt: str) -> Path:
    """The format's signature followed by garbage."""
    magic = {
        "png": b"\x89PNG\r\n\x1a\n", "jpg": b"\xff\xd8\xff\xe0", "gif": b"GIF89a",
        "webp": b"RIFF\x00\x01\x00\x00WEBPVP8 ", "bmp": b"BM", "tiff": b"II*\x00",
        "wav": b"RIFF\x00\x01\x00\x00WAVEfmt ", "mp3": b"ID3\x04\x00\x00\x00\x00\x00\x00",
        "flac": b"fLaC", "mp4": b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isom",
        "docx": b"PK\x03\x04", "xlsx": b"PK\x03\x04", "epub": b"PK\x03\x04",
    }.get(fmt, b"")  # fmt: skip
    path.write_bytes(magic + bytes((i * 37 + 11) % 256 for i in range(4096)))
    return path


def truncated(path: Path, fmt: str, make: Callable[[Path], Path]) -> Path:
    full = make(path.with_name(f"full-{path.name}"))
    data = full.read_bytes()
    path.write_bytes(data[: max(64, len(data) // 2)])
    full.unlink(missing_ok=True)
    return path


# ------------------------------------------------------------------ registry

IMAGE_FORMATS = ["png", "jpg", "webp", "heic", "gif", "bmp", "tiff", "avif", "ico", "svg"]
AUDIO_FORMATS = ["mp3", "wav", "flac", "ogg", "opus", "m4a", "aac", "aiff"]
VIDEO_FORMATS = ["mp4", "mov", "mkv", "webm", "avi"]
TEXT_FORMATS = ["txt", "md", "html", "csv", "json", "yaml", "rtf"]

BUILDERS: dict[str, tuple[str, Callable[[Path], Path]]] = {}
"""name → (extension, builder)."""

for _f in IMAGE_FORMATS:
    BUILDERS[f"image-{_f}"] = (_f, lambda p, f=_f: image_file(p, f))
for _f in AUDIO_FORMATS:
    BUILDERS[f"audio-{_f}"] = (_f, lambda p, f=_f: audio_file(p, f))
for _f in VIDEO_FORMATS:
    BUILDERS[f"video-{_f}"] = (_f, lambda p, f=_f: video_file(p, f))
for _f in TEXT_FORMATS:
    BUILDERS[f"doc-{_f}"] = (_f, lambda p, f=_f: text_file(p, f))
BUILDERS["doc-docx"] = ("docx", docx_file)
BUILDERS["doc-xlsx"] = ("xlsx", xlsx_file)
BUILDERS["doc-epub"] = ("epub", epub_file)
BUILDERS["doc-md-long"] = ("md", long_markdown)
BUILDERS["doc-csv-long"] = ("csv", long_csv)
BUILDERS["doc-pptx"] = ("pptx", slides_file)
BUILDERS["doc-pdf-report"] = ("pdf", report_pdf)
BUILDERS["doc-pdf-scan"] = ("pdf", scan_pdf)
for _f in OFFICE_FROM:
    BUILDERS[f"doc-{_f}"] = (_f, lambda p, f=_f: office_file(p, f))
BUILDERS["image-big-png"] = ("png", big_png)
BUILDERS["audio-long-wav"] = ("wav", lambda p: wav_tone(p, seconds=600, rate=48000))
BUILDERS["video-long-mp4"] = ("mp4", lambda p: video_file(p, "mp4", seconds=120, size="1280x720"))
BUILDERS["video-silent-mp4"] = ("mp4", silent_video)
for _f in [
    "png",
    "jpg",
    "gif",
    "webp",
    "bmp",
    "tiff",
    "wav",
    "mp3",
    "flac",
    "mp4",
    "docx",
    "xlsx",
    "epub",
]:
    BUILDERS[f"corrupt-{_f}"] = (_f, lambda p, f=_f: corrupt(p, f))
for _f in ["png", "jpg", "gif", "webp", "tiff"]:
    BUILDERS[f"truncated-{_f}"] = (_f, lambda p, f=_f: truncated(p, f, lambda q: image_file(q, f)))
for _f in ["wav", "mp3", "flac"]:
    BUILDERS[f"truncated-{_f}"] = (_f, lambda p, f=_f: truncated(p, f, lambda q: audio_file(q, f)))
BUILDERS["truncated-mp4"] = ("mp4", lambda p: truncated(p, "mp4", lambda q: video_file(q, "mp4")))
BUILDERS["truncated-docx"] = ("docx", lambda p: truncated(p, "docx", docx_file))
BUILDERS["truncated-xlsx"] = ("xlsx", lambda p: truncated(p, "xlsx", xlsx_file))
BUILDERS["truncated-epub"] = ("epub", lambda p: truncated(p, "epub", epub_file))


def build(name: str, directory: Path) -> Path:
    ext, make = BUILDERS[name]
    path = directory / f"{name}.{ext}"
    if not path.exists():
        directory.mkdir(parents=True, exist_ok=True)
        make(path)
    return path


def needs_soffice(name: str) -> bool:
    return name in ("doc-pptx", "truncated-pptx") or name.removeprefix("doc-") in OFFICE_FROM


def needs_ffmpeg(name: str) -> bool:
    ext = BUILDERS[name][0] if name in BUILDERS else ""
    return name.startswith(("video-", "truncated-mp", "truncated-flac")) or (
        name.startswith("audio-") and ext != "wav"
    )


__all__ = ["BUILDERS", "build", "engine_env", "ffmpeg", "needs_ffmpeg", "needs_soffice", "soffice"]
