"""Verifier for file-clean: is any identifying detail left in an output?

Runs in a **fresh process** (``python -I``) started by ``adapter.py``, in the
core's separate check job. It sees only the output's job-relative path, on
stdin as JSON (``{"output": "out/0.pdf"}``), and prints a JSON report::

    {"kinds": ["author", "location"]}

It re-reads the output with ``converter_sdk.reveal``, the same reader the
cleaning tool's own check uses, so a miss in that reader is a miss here too;
what this adds is a second process, in a second job, that the cleaning
tool's worker can't write the answer of. ``kinds`` are the fixed words of
``reveal`` (``author``, ``location``, ...), never a value or document text.
A failure to read the output is ``{"error": "<code>"}`` and counts as a
failed verification.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path, PurePosixPath

# What file-clean promises to leave empty (its own ``RIGHTS``): who, where,
# when and with what the file was made, plus tags and embedded files.
LEFT_BEHIND = {"location", "device", "author", "dates", "externalLinks", "tags", "embeddedFiles"}


def main() -> None:
    try:
        request = json.loads(sys.stdin.read())
        relative = PurePosixPath(str(request["output"]))
        if relative.is_absolute() or ".." in relative.parts:
            sys.stdout.write(json.dumps({"error": "badPath"}))
            return
        path = Path.cwd().joinpath(*relative.parts)
        fmt = path.suffix.lstrip(".").lower()
        from converter_sdk import reveal  # noqa: PLC0415

        if fmt in reveal.IMAGE_FORMATS:
            found = reveal.image_findings(path)
        elif fmt == "pdf":
            import pikepdf  # noqa: PLC0415

            with pikepdf.open(path) as pdf:
                found = reveal.pdf_findings(pdf)
        elif fmt in reveal.OOXML_FORMATS:
            found = reveal.ooxml_findings(path, external=True)
        else:
            # A format this verifier doesn't read is not one it can pass.
            sys.stdout.write(json.dumps({"error": "unsupportedFormat"}))
            return
        kinds = sorted({f.kind for f in found} & LEFT_BEHIND)
    except Exception as e:  # noqa: BLE001 - any failure to read is a failed verification
        sys.stdout.write(json.dumps({"error": getattr(e, "code", "verifierFailed")}))
        return
    sys.stdout.write(json.dumps({"kinds": kinds}))


if __name__ == "__main__":
    main()
