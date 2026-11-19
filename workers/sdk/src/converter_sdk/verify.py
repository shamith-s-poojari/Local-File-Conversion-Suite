"""Running an adapter's independent output verifier in a fresh process.

``pdf-redact`` and ``file-disarm`` each ship a ``verify.py`` that re-opens
their output with nothing from the job in memory. This module starts it with
``python -I``, the minimal helper environment and the output's job-relative
path on stdin, and returns what it reported.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

from converter_sdk.context import JobContext
from converter_sdk.engines import helper_env
from converter_sdk.errors import AdapterError

DEFAULT_TIMEOUT_S = 300
MAX_REPORT_BYTES = 4 * 1024 * 1024


def run_verifier(
    ctx: JobContext,
    script: Path,
    output: Path,
    extra: dict[str, Any] | None = None,
    *,
    timeout_s: float = DEFAULT_TIMEOUT_S,
) -> dict[str, Any] | None:
    """Run ``script`` on ``output``; return its JSON report, or ``None`` if it
    couldn't report. Callers must treat ``None`` as a failed verification,
    never a pass, and check the report's shape themselves.

    Raises ``E_CANCELLED`` when the job is cancelled and ``E_TIMEOUT`` after
    ``timeout_s``.
    """
    request = json.dumps(
        {**(extra or {}), "output": output.relative_to(ctx.job_dir.resolve()).as_posix()}
    )
    proc = subprocess.Popen(  # noqa: S603 - fixed argv: our interpreter and our verifier
        [sys.executable, "-I", str(script)],
        cwd=ctx.job_dir,
        env=helper_env(),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        # Read and dropped, not DEVNULL: a Windows AppContainer is denied the
        # null device. It can hold document text, so it's never kept.
        stderr=subprocess.PIPE,
    )
    if proc.stdin is None or proc.stdout is None or proc.stderr is None:  # pragma: no cover
        raise AdapterError("E_INTERNAL", {"reason": "verifierPipes"})
    err, out = proc.stderr, proc.stdout
    chunks: list[bytes] = []
    size = 0

    def drop_stderr() -> None:
        while err.read(65536):
            pass

    def collect_stdout() -> None:
        # Read while the verifier runs: a report larger than the pipe would
        # otherwise block it until the timeout. Past the cap it's drained and
        # dropped, and the report is rejected below.
        nonlocal size
        while chunk := out.read(65536):
            size += len(chunk)
            if size <= MAX_REPORT_BYTES:
                chunks.append(chunk)

    threading.Thread(target=drop_stderr, daemon=True).start()
    reader = threading.Thread(target=collect_stdout, daemon=True)
    reader.start()
    try:
        proc.stdin.write(request.encode("utf-8"))
        proc.stdin.close()
    except OSError:
        pass  # it exited without reading; what it printed (if anything) is the report
    deadline = time.monotonic() + timeout_s
    while proc.poll() is None:
        if ctx.cancelled or time.monotonic() > deadline:
            proc.kill()
            proc.wait(5)
            ctx.check_cancelled()
            raise AdapterError("E_TIMEOUT", {"stage": "verify"})
        time.sleep(0.05)
    reader.join(5)
    if size > MAX_REPORT_BYTES:
        return None
    try:
        report = json.loads(b"".join(chunks))
    except ValueError:
        return None
    return report if isinstance(report, dict) else None
