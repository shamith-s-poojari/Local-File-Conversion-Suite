"""Verifier for file-disarm: is any active content left in an output?

Runs in a **fresh process** (``python -I``) started by ``adapter.py``. It
sees only the output's job-relative path, on stdin as JSON
(``{"output": "out/0.docx"}``), and prints a JSON report on stdout::

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
SUSPECT_PARTS = ("vbaproject", "vbadata", "/activex/", "/embeddings/", "xl/externallinks/")


def report(findings: list[dict[str, str | None]]) -> None:
    sys.stdout.write(json.dumps({"findings": findings}))


def fail(code: str) -> None:
    sys.stdout.write(json.dumps({"error": code}))


def raw_part_findings(path: Path) -> list[dict[str, str | None]]:
    import zipfile  # noqa: PLC0415

    with zipfile.ZipFile(path) as z:
        names = ["/" + n.replace("\\", "/").lower() for n in z.namelist()]
    hits = [s for s in SUSPECT_PARTS if any(s in n for n in names)]
    return [{"kind": "activeContent", "detail": "Part names left"}] if hits else []


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
    except Exception as e:  # noqa: BLE001 - any failure to read is a failed verification
        fail(getattr(e, "code", "verifierFailed"))
        return
    report(found)


if __name__ == "__main__":
    main()
