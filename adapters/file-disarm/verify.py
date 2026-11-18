"""Verifier for file-disarm: is any active content left in an output?

Runs in a **fresh process** (``python -I``) started by ``adapter.py``. It
sees only the output's job-relative path, on stdin as JSON
(``{"output": "out/0.docx", "removeLinks": false}``), and prints a JSON report on stdout::

    {"findings": [{"kind": "macros", "detail": "Macros (VBA)"}, ...]}

It opens the output fresh with ``converter_sdk.active`` and, for OOXML,
also looks at the package's part names without parsing anything, so one
reader's blind spot doesn't pass silently. A report never contains document
text, only kinds and counts. A failure to read the output is reported as
``{"error": "<code>"}`` and counts as a failed verification.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path, PurePosixPath

OOXML = {"docx", "xlsx", "pptx", "docm", "xlsm", "pptm"}
SUSPECT_PARTS = (
    "vbaproject",
    "vbadata",
    "/activex/",
    "/embeddings/",
    "xl/externallinks/",
    "/xl/connections.xml",
    "/xl/querytables/",
    "/customxml/",
    "/customui/",
)
IMPORT_RELATIONSHIP = b"/afchunk"
"""A relationship type (lower-cased) that makes Word open another document with this one."""


def report(findings: list[dict[str, str | None]]) -> None:
    sys.stdout.write(json.dumps({"findings": findings}))


def fail(code: str) -> None:
    sys.stdout.write(json.dumps({"error": code}))


def raw_part_findings(path: Path) -> list[dict[str, str | None]]:
    import zipfile  # noqa: PLC0415

    with zipfile.ZipFile(path) as z:
        names = ["/" + n.replace("\\", "/").lower() for n in z.namelist()]
        imports = any(
            n.lower().endswith(".rels") and IMPORT_RELATIONSHIP in z.read(n).lower()
            for n in z.namelist()
            if z.getinfo(n).file_size <= 8 * 1024 * 1024
        )
    hits = [s for s in SUSPECT_PARTS if any(s in n for n in names)]
    return [{"kind": "activeContent", "detail": "Part names left"}] if hits or imports else []


PDF_ACTIVE_NAMES = {
    "/Launch",
    "/JavaScript",
    "/JS",
    "/GoToR",
    "/GoToE",
    "/SubmitForm",
    "/ImportData",
    "/RichMedia",
}
"""Names that mean something runs or calls out, as a key or as a value (``/S /Launch``)."""


def raw_pdf_findings(path: Path, remove_links: bool) -> list[dict[str, str | None]]:
    """Walk every object of the output, with nothing from the SDK's detector, and
    report what is still there: kinds only."""
    import pikepdf  # noqa: PLC0415
    from converter_sdk.urls import is_web_url  # noqa: PLC0415

    kinds: set[str] = set()
    with pikepdf.open(path) as pdf:
        for obj in pdf.objects:
            stack = [obj.stream_dict if isinstance(obj, pikepdf.Stream) else obj]
            budget = 500_000
            while stack and budget > 0:
                budget -= 1
                node = stack.pop()
                if isinstance(node, pikepdf.Name):
                    if str(node) in PDF_ACTIVE_NAMES:
                        kinds.add(str(node).lstrip("/"))
                    continue
                if isinstance(node, pikepdf.Dictionary):
                    for key in node.keys():
                        if key in PDF_ACTIVE_NAMES:
                            kinds.add(key.lstrip("/"))
                        if key == "/URI":
                            uri = node[key]
                            if isinstance(uri, pikepdf.Dictionary):
                                continue  # the catalog's /URI /Base: a base, not a link
                            try:
                                web = isinstance(uri, pikepdf.String) and is_web_url(str(uri))
                            except (UnicodeDecodeError, ValueError):
                                web = False
                            if remove_links or not web:
                                kinds.add("URI")
                    children = [node[k] for k in node.keys()]
                elif isinstance(node, pikepdf.Array):
                    children = list(node)
                else:
                    continue
                stack.extend(
                    c
                    for c in children
                    if isinstance(c, (pikepdf.Dictionary, pikepdf.Array, pikepdf.Name))
                    and not c.is_indirect
                )
    return [{"kind": "activeContent", "detail": f"PDF {k} left"} for k in sorted(kinds)]


def main() -> None:
    try:
        request = json.loads(sys.stdin.read())
        relative = PurePosixPath(str(request["output"]))
        if relative.is_absolute() or ".." in relative.parts:
            fail("badPath")
            return
        path = Path.cwd().joinpath(*relative.parts)
        fmt = path.suffix.lstrip(".").lower()
        from converter_sdk.active import active_findings  # noqa: PLC0415

        found = [{"kind": f.kind, "detail": f.detail} for f in active_findings(path, fmt)]
        if fmt in OOXML:
            found += raw_part_findings(path)
        if fmt == "pdf":
            found += raw_pdf_findings(path, request.get("removeLinks") is True)
    except Exception as e:  # noqa: BLE001 - any failure to read is a failed verification
        fail(getattr(e, "code", "verifierFailed"))
        return
    report(found)


if __name__ == "__main__":
    main()
