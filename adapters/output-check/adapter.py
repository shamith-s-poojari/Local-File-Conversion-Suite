"""output-check: runs a tool's own verifier over its outputs, for the core.

The core starts this in a separate job after a tool that declares
``recheck`` has finished (ADR 0006), so the tool's worker can't approve its
own output. Everything it is told (``params``) comes from the core. It
loads no code the request names: ``verifier`` picks one of three fixed
paths: two in the tools' adapter folders, one beside this file.

The report in ``out/report.json`` holds check IDs, pages and counts only,
never document text. It is deterministic, so a repeat run is identical.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.active import active_findings
from converter_sdk.adapter import issue
from converter_sdk.convert import source_format
from converter_sdk.limits import MB
from converter_sdk.protocol import ValidationIssue
from converter_sdk.verify import run_verifier

ADAPTERS = Path(__file__).resolve().parents[1]
VERIFIERS = {
    "redaction": ADAPTERS / "pdf-redact" / "verify.py",
    "activeContent": ADAPTERS / "file-disarm" / "verify.py",
    # The only verifier that lives here: file-clean (Remove hidden details)
    # declares no `recheck` in its manifest, the core asks for this one by
    # the adapter's id.
    "hiddenDetails": Path(__file__).resolve().parent / "verify_details.py",
}
VERIFY_TIMEOUT_S = 600
REDACTION_REQUEST = ("expectedPages", "fill", "terms", "regions")
"""What the core may tell the redaction verifier. Never ``captured``: the core
has no text from under the boxes, and the tool's own worker checks that."""


def _failed(check_id: str, **extra: Any) -> dict[str, Any]:
    return {"id": check_id, "passed": False, **extra}


def redaction_checks(report: dict[str, Any] | None) -> list[dict[str, Any]]:
    """The verifier's checks, or one failed check if it couldn't report."""
    checks = report.get("checks") if report else None
    if not isinstance(checks, list) or not checks:
        return [_failed("V0", reason="verifierFailed")]
    out = []
    for c in checks:
        if not isinstance(c, dict) or not isinstance(c.get("id"), str):
            return [_failed("V0", reason="verifierFailed")]
        entry: dict[str, Any] = {"id": c["id"], "passed": c.get("passed") is True}
        # Page numbers and counts only: a reason such as "hiddenLayers" is a
        # fixed word from the verifier, document text never is.
        for key in ("pages", "reasons"):
            if isinstance(c.get(key), list):
                entry[key] = c[key][:100]
        out.append(entry)
    return out


def active_content_checks(report: dict[str, Any] | None) -> list[dict[str, Any]]:
    findings = report.get("findings") if report else None
    if not isinstance(findings, list):
        return [_failed("verifierFailed")]
    return [{"id": "activeContent", "passed": not findings, "count": len(findings)}]


HIDDEN_KINDS = {"location", "device", "author", "dates", "externalLinks", "tags", "embeddedFiles"}
"""The words ``verify_details.py`` may report; anything else is dropped."""


def hidden_details_checks(report: dict[str, Any] | None) -> list[dict[str, Any]]:
    kinds = report.get("kinds") if report else None
    if not isinstance(kinds, list) or not all(isinstance(k, str) for k in kinds):
        return [_failed("verifierFailed")]
    left = sorted(set(kinds) & HIDDEN_KINDS)
    # An unknown word is still something left behind: fail without echoing it.
    unknown = any(k not in HIDDEN_KINDS for k in kinds)
    return [
        {
            "id": "hiddenDetails",
            "passed": not left and not unknown,
            "count": len(kinds),
            **({"kinds": left} if left else {}),
        }
    ]


class OutputCheckAdapter(Adapter):
    adapter_id: ClassVar[str] = "output-check"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        report = ValidationReport()
        if params.get("verifier") == "redaction" and len(ctx.inputs) != 1:
            report.errors.append(issue("inputs", "exactlyOne"))
        total = 0
        for path in ctx.inputs:
            ctx.check_cancelled()
            relative = path.relative_to(ctx.job_dir.resolve()).as_posix()
            if not path.is_file():
                report.errors.append(ValidationIssue(code="E_INPUT_CORRUPT", input=relative))
                continue
            total += path.stat().st_size
            fmt = source_format(path)
            try:
                if fmt == "pdf":
                    with ctx.open_pdf(path):
                        pass
                else:
                    active_findings(path, fmt)
            except AdapterError as e:
                report.errors.append(ValidationIssue(code=e.code, input=relative, detail=e.detail))
        report.metadata = {"inputBytes": total}
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        size = int(metadata.get("inputBytes", 0))
        return ResourceEstimate(
            ram_bytes=256 * MB + 2 * size,
            disk_bytes=size,
            cost_units=max(1, len(ctx.inputs)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        kind = params["verifier"]
        script = VERIFIERS[kind]
        extra = (
            {k: params[k] for k in REDACTION_REQUEST if k in params}
            if kind == "redaction"
            else None
        )
        inputs = []
        n = len(ctx.inputs)
        ctx.stage("check", 1, 1)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.progress("check", i, n)
            report = run_verifier(ctx, script, path, extra, timeout_s=VERIFY_TIMEOUT_S)
            if kind == "redaction":
                checks = redaction_checks(report)
            elif kind == "hiddenDetails":
                checks = hidden_details_checks(report)
            else:
                checks = active_content_checks(report)
            inputs.append({"input": i, "checks": checks})
        ctx.progress("check", n, n)

        failed = sorted({c["id"] for r in inputs for c in r["checks"] if not c["passed"]})
        body = {"verifier": kind, "passed": not failed, "failed": failed, "inputs": inputs}
        out = ctx.output_path("report.json")
        out.write_text(json.dumps(body, sort_keys=True), encoding="utf-8")
        return [OutputFile(out)]


if __name__ == "__main__":
    run_main(OutputCheckAdapter)
