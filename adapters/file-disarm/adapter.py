"""file-disarm: make a file safe to open (content disarm and reconstruction).

Removes what can run, call out or hide a payload when a file is opened:
scripts, macros, ActiveX, embedded files and objects, automatic actions,
and links that load things from outside the file. Plain web links stay
unless ``removeLinks`` is set. Each file keeps its format; a macro-enabled
Office file (``docm``, ``xlsm``, ``pptm``) is saved as the plain one.

**A false "safe" is worse than a failed job.** So every output is checked
by ``verify.py`` in a fresh process; if anything active is left, the output
is deleted and the job fails with ``E_INTERNAL`` (reason ``activeContentRemains``).

The removal code is in ``converter_sdk.pdf`` (PDF) and ``converter_sdk.active``
(Office files, SVG, HTML).
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    AdapterError,
    InspectReport,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.active import OOXML_FORMATS, active_findings, disarm_markup, disarm_ooxml
from converter_sdk.convert import ConversionAdapter, issue, source_format
from converter_sdk.engines import helper_env
from converter_sdk.pdf import flatten_forms, is_signed, remove_active_content, save_pdf
from converter_sdk.protocol import ValidationIssue

MB = 1024 * 1024
VERIFY = Path(__file__).resolve().with_name("verify.py")
VERIFY_TIMEOUT_S = 300
SAME_AS = {"docm": "docx", "xlsm": "xlsx", "pptm": "pptx"}
FORMATS = {"pdf", "svg", "html", *OOXML_FORMATS}

# Which counts from the removers mean which warning.
MACROS = {"macros", "macroButtons"}
EXTERNAL = {"external", "fieldLinks", "workbookLinks", "links", "remote"}


def output_format(fmt: str) -> str:
    return SAME_AS.get(fmt, fmt)


# ------------------------------------------------------------ verifying


def run_verifier(ctx: JobContext, output: Path) -> dict[str, Any]:
    """Run ``verify.py`` in a fresh process. It sees only the output's
    job-relative path, on stdin."""
    env = helper_env()
    request = json.dumps({"output": output.relative_to(ctx.job_dir.resolve()).as_posix()})
    proc = subprocess.Popen(  # noqa: S603 - fixed argv: our interpreter and our verifier
        [sys.executable, "-I", str(VERIFY)],
        cwd=ctx.job_dir,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        # Read and dropped, not DEVNULL: a Windows AppContainer is denied the
        # null device. It can hold document text, so it's never kept.
        stderr=subprocess.PIPE,
    )
    if proc.stdin is None or proc.stdout is None or proc.stderr is None:  # pragma: no cover
        raise AdapterError("E_INTERNAL", {"reason": "verifierPipes"})
    err = proc.stderr

    def drop_stderr() -> None:
        while err.read(65536):
            pass

    threading.Thread(target=drop_stderr, daemon=True).start()
    proc.stdin.write(request.encode("utf-8"))
    proc.stdin.close()
    deadline = time.monotonic() + VERIFY_TIMEOUT_S
    while proc.poll() is None:
        if ctx.cancelled or time.monotonic() > deadline:
            proc.kill()
            proc.wait(5)
            ctx.check_cancelled()
            raise AdapterError("E_TIMEOUT", {"stage": "verify"})
        time.sleep(0.05)
    raw = proc.stdout.read()
    try:
        report = json.loads(raw)
        if not isinstance(report.get("findings"), list):
            raise ValueError("no findings")
    except (ValueError, AttributeError):
        # A verifier that can't report is a failed verification, never a pass.
        return {"error": "verifierFailed"}
    return report


def check_output(ctx: JobContext, output: Path) -> None:
    """Fail the job, and delete the output, unless the verifier finds nothing active."""
    report = run_verifier(ctx, output)
    if "error" in report:
        output.unlink(missing_ok=True)
        raise AdapterError("E_INTERNAL", {"reason": "verifierFailed"}, retryable=False)
    kinds = sorted({str(f.get("kind")) for f in report["findings"]})
    if kinds:
        output.unlink(missing_ok=True)
        raise AdapterError(
            "E_INTERNAL", {"reason": "activeContentRemains", "kinds": kinds}, retryable=False
        )


# --------------------------------------------------------------- adapter


class FileDisarmAdapter(Adapter):
    adapter_id: ClassVar[str] = "file-disarm"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "H"

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        # The core's inspector for Office files and HTML: what the file
        # reveals and what it could do when opened (ADR 0005).
        from converter_sdk import reveal  # noqa: PLC0415

        return InspectReport(pages=0, encryption="none", findings=reveal.findings(ctx, input))

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        report = ValidationReport()
        total = 0
        for path in ctx.inputs:
            ctx.check_cancelled()
            relative = path.relative_to(ctx.job_dir.resolve()).as_posix()
            if not path.is_file():
                report.errors.append(ValidationIssue(code="E_INPUT_CORRUPT", input=relative))
                continue
            total += path.stat().st_size
            fmt = source_format(path)
            if fmt not in FORMATS:
                report.errors.append(issue("inputs", "unsupportedFormat"))
                continue
            try:
                if fmt == "pdf":
                    with ctx.open_pdf(path) as pdf:
                        if len(pdf.pages) == 0:
                            raise AdapterError("E_INPUT_CORRUPT")
                else:
                    active_findings(path, fmt)  # reads the file the way the removal will
            except AdapterError as e:
                report.errors.append(ValidationIssue(code=e.code, input=relative, detail=e.detail))
        report.metadata = {"inputBytes": total}
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        size = int(metadata.get("inputBytes", 0))
        return ResourceEstimate(
            ram_bytes=192 * MB + 2 * size,
            disk_bytes=3 * size,
            cost_units=max(1, len(ctx.inputs)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        remove_links = params.get("removeLinks") is True
        outputs: list[OutputFile] = []
        totals: dict[str, int] = {}
        flattened = signed = False
        n = len(ctx.inputs)
        ctx.stage("disarm", 1, 2)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.progress("disarm", i, n)
            fmt = source_format(path)
            title = ConversionAdapter.title(params, i)
            out = ctx.output_path(f"{title}-safe.{output_format(fmt)}")
            pages: int | None = None
            if fmt == "pdf":
                with ctx.open_pdf(path) as pdf:
                    signed |= is_signed(pdf)
                    counts = remove_active_content(
                        pdf, remove_links=remove_links, check=ctx.check_cancelled
                    )
                    if counts.get("formActions"):
                        flatten_forms(pdf)
                        flattened = True
                    pages = save_pdf(ctx, pdf, out)
            elif fmt in OOXML_FORMATS:
                counts = disarm_ooxml(path, out, remove_links=remove_links)
            else:
                counts = disarm_markup(path, out, fmt, remove_links=remove_links)
            for key, value in counts.items():
                totals[key] = totals.get(key, 0) + value
            outputs.append(OutputFile(out, pages=pages, source=i))
        ctx.progress("disarm", n, n)

        ctx.stage("verify", 2, 2)
        for k, output in enumerate(outputs):
            ctx.check_cancelled()
            check_output(ctx, output.path)
            ctx.progress("verify", k + 1, n)

        self.warn(ctx, totals, flattened, signed)
        return outputs

    @staticmethod
    def warn(ctx: JobContext, totals: dict[str, int], flattened: bool, signed: bool) -> None:
        macros = sum(v for k, v in totals.items() if k in MACROS)
        external = sum(v for k, v in totals.items() if k in EXTERNAL)
        other = sum(v for k, v in totals.items() if k not in MACROS | EXTERNAL | {"formActions"})
        if macros:
            ctx.warn("MACROS_REMOVED", {"count": macros})
        if other:
            ctx.warn("ACTIVE_CONTENT_REMOVED", {"count": other})
        if external:
            ctx.warn("EXTERNAL_LINKS_REMOVED", {"count": external})
        if flattened:
            ctx.warn("FORMS_FLATTENED", {"removed": True})
        if signed:  # the file is always rewritten, so a signature can't survive
            ctx.warn("SIGNATURE_INVALIDATED")


if __name__ == "__main__":
    run_main(FileDisarmAdapter)
