"""Run adapters the way the core does, and build in-process contexts for unit tests.

``WorkerProcess`` starts ``python -I adapter.py <jobId>`` in a job directory
and speaks the protocol, so conformance checks exercise the same process
rules as production (spec section 7.1). It runs without a sandbox; the
sandbox is the core's job and is tested by the probe (spec section 9.3).
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from converter_sdk import JobContext
from converter_sdk.protocol import Secrets, parse_worker_message

JOB_ID = "j_conformance"


def make_job_dir(root: Path, inputs: list[Path]) -> tuple[Path, list[str]]:
    """Stage inputs as ``in/<n>.pdf`` (copies, spec change C3). Returns job-relative paths."""
    root.mkdir(parents=True, exist_ok=True)
    for d in ("in", "out", "work"):
        (root / d).mkdir(exist_ok=True)
    relative = []
    for n, src in enumerate(inputs):
        dst = root / "in" / f"{n}{src.suffix or '.bin'}"
        shutil.copyfile(src, dst)
        relative.append(dst.relative_to(root).as_posix())
    return root, relative


def context(
    job_dir: Path, inputs: list[str], secrets: dict[str, str] | None = None
) -> tuple[JobContext, list[Any]]:
    """An in-process ``JobContext``; the returned list collects sent messages."""
    sent: list[Any] = []
    ctx = JobContext(
        job_id=JOB_ID,
        job_dir=job_dir,
        inputs=[(job_dir / p).resolve() for p in inputs],
        secrets=Secrets(**secrets) if secrets else None,
        _send=sent.append,
        _cancel=threading.Event(),
    )
    return ctx, sent


@dataclass
class RunResult:
    messages: list[Any]
    stderr: bytes
    peak_rss_bytes: int | None
    lines: list[bytes] = field(default_factory=list)

    def of(self, type_: str) -> list[Any]:
        return [m for m in self.messages if m.type == type_]

    def last(self, type_: str) -> Any | None:
        found = self.of(type_)
        return found[-1] if found else None


class WorkerProcess:
    def __init__(self, adapter: Path, job_dir: Path) -> None:
        self.p = subprocess.Popen(  # noqa: S603 - fixed argv, our own adapter
            [sys.executable, "-I", str(Path(adapter).resolve()), JOB_ID],
            cwd=job_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.lines: list[bytes] = []
        self._stderr: list[bytes] = []
        self._readers = [
            threading.Thread(target=self._read, daemon=True),
            threading.Thread(target=self._read_err, daemon=True),
        ]
        for t in self._readers:
            t.start()
        self.peak_rss_bytes: int | None = None

    def _read(self) -> None:
        assert self.p.stdout is not None
        for line in self.p.stdout:
            self.lines.append(line)

    def _read_err(self) -> None:
        assert self.p.stderr is not None
        self._stderr.append(self.p.stderr.read())

    @property
    def stderr(self) -> bytes:
        return b"".join(self._stderr)

    def send(self, message: dict[str, Any]) -> None:
        assert self.p.stdin is not None
        line = json.dumps({"v": 1, "jobId": JOB_ID, **message}, ensure_ascii=False)
        self.p.stdin.write(line.encode("utf-8") + b"\n")
        self.p.stdin.flush()

    def messages(self) -> list[Any]:
        return [parse_worker_message(line) for line in list(self.lines)]

    def wait_for(self, *types: str, timeout: float = 60, after: int = 0) -> Any:
        """The first message at or after index ``after`` whose type is in ``types``."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for m in self.messages()[after:]:
                if m.type in types:
                    return m
            # stdout closing means the worker is gone. poll() would reap it
            # and lose the resource usage that finish() reads.
            if not self._readers[0].is_alive():
                for m in self.messages()[after:]:
                    if m.type in types:
                        return m
                raise AssertionError(f"worker closed stdout before {types}")
            time.sleep(0.01)
        raise AssertionError(f"no {types} within {timeout}s")

    def finish(self, timeout: float = 5) -> None:
        """Close stdin, reap the process and record its peak RSS where the OS reports it."""
        if self.p.stdin is not None and not self.p.stdin.closed:
            self.p.stdin.close()
        deadline = time.monotonic() + timeout
        if hasattr(os, "wait4"):
            while time.monotonic() < deadline:
                try:
                    pid, status, usage = os.wait4(self.p.pid, os.WNOHANG)
                except ChildProcessError:  # already reaped elsewhere
                    break
                if pid:
                    self.p.returncode = os.waitstatus_to_exitcode(status)
                    scale = 1 if sys.platform == "darwin" else 1024  # bytes vs KiB
                    self.peak_rss_bytes = usage.ru_maxrss * scale
                    break
                time.sleep(0.02)
            else:
                self.kill()
        else:
            try:
                self.p.wait(timeout)
            except subprocess.TimeoutExpired:
                self.kill()
        for t in self._readers:
            t.join(2)

    def kill(self) -> None:
        if self.p.poll() is None:
            self.p.kill()
            self.p.wait(5)


def run_job(
    adapter: Path,
    job_dir: Path,
    inputs: list[str],
    params: dict[str, Any],
    secrets: dict[str, str] | None = None,
    *,
    timeout: float = 120,
) -> RunResult:
    """validate → estimate → execute, stopping at the first failure, like the scheduler."""
    w = WorkerProcess(adapter, job_dir)
    try:
        w.wait_for("hello", timeout=10)
        extra = {"secrets": secrets} if secrets else {}
        w.send({"type": "validate", "inputs": inputs, "params": params, **extra})
        v = w.wait_for("validateResult", "error", timeout=timeout)
        if v.type == "validateResult" and not v.errors:
            w.send(
                {
                    "type": "estimate",
                    "inputs": inputs,
                    "params": params,
                    "metadata": v.metadata.root,
                }
            )
            e = w.wait_for("estimateResult", "error", timeout=timeout)
            if e.type == "estimateResult":
                w.send(
                    {
                        "type": "execute",
                        "inputs": inputs,
                        "outDir": "out",
                        "workDir": "work",
                        "params": params,
                        **extra,
                    }
                )
                w.wait_for("executeResult", "error", timeout=timeout, after=len(w.lines) - 1)
    finally:
        w.finish()
    return RunResult(w.messages(), w.stderr, w.peak_rss_bytes, list(w.lines))


def load_adapter_class(adapter_dir: Path) -> type:
    """Import ``adapter_dir/adapter.py`` and return its ``Adapter`` subclass."""
    import importlib.util  # noqa: PLC0415

    from converter_sdk import Adapter  # noqa: PLC0415

    name = "adapter_" + adapter_dir.name.replace("-", "_")
    spec = importlib.util.spec_from_file_location(name, adapter_dir / "adapter.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    found = [
        v
        for v in vars(module).values()
        if isinstance(v, type) and issubclass(v, Adapter) and v.__module__ == name
    ]
    assert len(found) == 1, f"{adapter_dir.name}: expected one Adapter subclass"
    return found[0]


@dataclass
class InProcessRun:
    errors: list[Any]
    validation_warnings: list[str]
    outputs: list[Path]
    names: list[str]
    warnings: list[str]
    sent: list[Any]


def run_in_process(
    adapter_dir: Path,
    job_dir: Path,
    inputs: list[Path],
    params: dict[str, Any],
    secrets: dict[str, str] | None = None,
) -> InProcessRun:
    """validate, then execute if validation passed, calling the adapter directly.

    Faster than ``run_job`` and gives tracebacks; use it for acceptance tests.
    Conformance (process rules) is covered by ``run_job``.
    """
    adapter = load_adapter_class(adapter_dir)()
    job_dir, rel = make_job_dir(job_dir, inputs)
    ctx, sent = context(job_dir, rel, secrets)
    report = adapter.validate_input(ctx, params)
    run = InProcessRun(
        errors=list(report.errors),
        validation_warnings=[w.code for w in report.warnings],
        outputs=[],
        names=[],
        warnings=[],
        sent=sent,
    )
    if report.errors:
        return run
    outputs = adapter.execute_conversion(ctx, params)
    run.outputs = [o.path for o in outputs]
    run.names = [ctx.suggested_name(o.path) for o in outputs]
    run.warnings = [m.code for m in sent if getattr(m, "type", None) == "warning"]
    return run
