"""LibreOffice, headless, for office documents (ADR 0003).

Each job gets its own LibreOffice profile under ``work/`` (``HOME`` is
``work/`` too), so nothing is read from or written to the user's real
profile. Macros never run: headless conversion uses the default "high"
macro security, and nothing asks for confirmation.
"""

from __future__ import annotations

import os
import shutil
from pathlib import Path

from .context import JobContext
from .engines import binary, run
from .errors import AdapterError

TIMEOUT_S = 300
RESTART_REQUESTED = 81
"""soffice.bin's exit code after it creates a user profile: start it again.
The `soffice` script (oosplash) does that for its callers."""
"""One document. A long novel converts in seconds; a hang is killed."""

# LibreOffice export filters by target extension and document family.
WRITER_FILTERS = {
    "pdf": "pdf:writer_pdf_Export",
    "docx": "docx:MS Word 2007 XML",
    "doc": "doc:MS Word 97",
    "odt": "odt:writer8",
    "rtf": "rtf:Rich Text Format",
    "txt": "txt:Text (encoded):UTF8",
    "html": "html:XHTML Writer File:UTF8",
    "epub": "epub:EPUB",
}
IMPRESS_FILTERS = {
    "pdf": "pdf:impress_pdf_Export",
    "pptx": "pptx:Impress MS PowerPoint 2007 XML",
    "ppt": "ppt:MS PowerPoint 97",
    "odp": "odp:impress8",
}
CALC_FILTERS = {
    "pdf": "pdf:calc_pdf_Export",
    "xlsx": "xlsx:Calc MS Excel 2007 XML",
    "xls": "xls:MS Excel 97",
    "ods": "ods:calc8",
    "html": "html:HTML (StarCalc)",
    # Field separator, text delimiter, UTF-8, first line 1, (formats), language,
    # quote all text, detect special numbers, save as shown, export formulas,
    # remove spaces, then -1: every sheet into its own file.
    "csv": "csv:Text - txt - csv (StarCalc):44,34,76,1,,0,false,true,true,false,false,-1",
}
IMPORT_FILTERS = {
    # Comma, double quote, UTF-8, from line 1, …, and token 13 false: a cell
    # like "=HYPERLINK(…)" stays text instead of becoming a live formula.
    "csv": "CSV:44,34,76,1,,0,false,true,true,false,false,0,false",
    "html": "HTML (StarWriter)",
    "txt": "Text (encoded):UTF8",
}


CFB_MAGIC = bytes.fromhex("D0CF11E0A1B11AE1")
"""Start of a Compound File (OLE2) container."""

OOXML_FORMATS = frozenset({"docx", "pptx", "xlsx"})
"""Formats that are zip packages. The older ``doc``/``ppt``/``xls`` are
Compound Files legitimately, so they don't count."""


def is_encrypted_ooxml(path: Path, fmt: str) -> bool:
    """Whether a ``docx``/``pptx``/``xlsx`` is really a Compound File.

    Office wraps a password-protected package in one (an ``EncryptedPackage``
    stream), so the zip a reader expects isn't there. Reading it as
    "corrupt" would send the user to repair a file that only needs its
    password."""
    if fmt not in OOXML_FORMATS:
        return False
    try:
        with path.open("rb") as f:
            return f.read(len(CFB_MAGIC)) == CFB_MAGIC
    except OSError:
        return False


def reject_encrypted_ooxml(path: Path, fmt: str) -> None:
    """Raise ``E_PASSWORD_REQUIRED`` for a password-protected Office file.
    The SDK can't open these (no password goes to LibreOffice headless)."""
    if is_encrypted_ooxml(path, fmt):
        raise AdapterError("E_PASSWORD_REQUIRED", {"reason": "officePassword"}, retryable=False)


def soffice_convert(
    ctx: JobContext,
    src: Path,
    filter_spec: str,
    *,
    import_filter: str | None = None,
) -> list[Path]:
    """Convert ``src`` with an export filter (``"pdf:writer_pdf_Export"``).

    Returns the files LibreOffice wrote, in the order it wrote them (more
    than one for a spreadsheet saved as CSV: one per sheet, in workbook order).
    """
    exe = binary("soffice")
    profile = ctx.work_dir / "lo-profile"
    outdir = ctx.work_dir / f"lo-out-{os.getpid()}-{len(list(ctx.work_dir.glob('lo-out-*')))}"
    outdir.mkdir(parents=True, exist_ok=True)
    argv: list[object] = [
        exe,
        f"-env:UserInstallation={profile.resolve().as_uri()}",
        "--headless",
        "--invisible",
        "--nologo",
        "--norestore",
        "--nodefault",
        "--nolockcheck",
        "--nofirststartwizard",
    ]
    if import_filter:
        argv.append(f"--infilter={import_filter}")
    argv += ["--convert-to", filter_spec, "--outdir", outdir, src]
    work = str(ctx.work_dir)
    env = {"HOME": work, "TMPDIR": work, "TEMP": work, "TMP": work}
    code, _err = run(ctx, argv, env=env, timeout_s=TIMEOUT_S)
    if code == RESTART_REQUESTED:
        # A new profile was just made; LibreOffice's launcher would start it
        # again, but the sandbox runs soffice.bin directly, so do it here.
        code, _err = run(ctx, argv, env=env, timeout_s=TIMEOUT_S)
    produced = sorted(
        (p for p in outdir.iterdir() if p.is_file()), key=lambda p: (p.stat().st_mtime_ns, p.name)
    )
    if code != 0 or not produced:
        shutil.rmtree(outdir, ignore_errors=True)
        # LibreOffice says nothing useful on stderr for damaged input; it
        # just writes no file. Locked (password) documents end the same way.
        raise AdapterError("E_INPUT_CORRUPT", {"reason": "officeCouldNotOpen"})
    return produced


def family(fmt: str) -> str:
    if fmt in ("pptx", "ppt", "odp"):
        return "impress"
    if fmt in ("xlsx", "xls", "ods", "csv"):
        return "calc"
    return "writer"


def export_filter(source: str, target: str) -> str:
    table = {"impress": IMPRESS_FILTERS, "calc": CALC_FILTERS, "writer": WRITER_FILTERS}[
        family(source)
    ]
    try:
        return table[target]
    except KeyError:
        raise AdapterError(
            "E_INTERNAL", {"reason": "unsupportedConversion"}, retryable=False
        ) from None
