"""In-process tests of the worker runtime, with a scripted adapter."""

from __future__ import annotations

import hashlib
import json
import mimetypes
import queue
import threading
import time
from pathlib import Path
from typing import Any

import pytest
from converter_sdk import (
    Adapter,
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    Worker,
    safe_display_name,
)
from converter_sdk.protocol import parse_worker_message

JOB = "j_test"
SECRET = "sentinel-password-7c1e"


class ScriptedAdapter(Adapter):
    adapter_id = "scripted"
    fidelity = "E"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        return ValidationReport(metadata={"inputs": len(ctx.inputs), "seen": params.get("x")})

    def estimate_memory(self, ctx, params, metadata) -> ResourceEstimate:
        return ResourceEstimate(ram_bytes=10, disk_bytes=20, cost_units=metadata["n"])

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        mode = params.get("mode", "ok")
        ctx.stage("work", 1, 1)
        if mode == "slow":
            for i in range(1000):
                ctx.check_cancelled()
                ctx.progress("work", i, 1000)
                time.sleep(0.01)
        if mode == "crash":
            raise ValueError(f"contains document text and {SECRET}")
        if mode == "fail":
            raise AdapterError("E_PASSWORD_WRONG", {"page": 3})
        if mode == "escape":
            outside = ctx.job_dir / "in" / "evil.pdf"
            outside.write_bytes(b"x")
            return [OutputFile(outside)]
        if mode == "unreserved":
            path = ctx.out_dir / "made-up.pdf"
            path.write_bytes(b"x")
            return [OutputFile(path)]
        if mode == "none":
            return []
        if mode == "flood":
            for i in range(10_000):
                ctx.progress("work", i, 10_000)
        out = ctx.output_path("Report (final).pdf")
        out.write_bytes(b"%PDF-1.7 hello")
        return [OutputFile(out, pages=1)]


class Harness:
    def __init__(self, job_dir: Path, adapter: Adapter | None = None) -> None:
        self.lines: queue.Queue[bytes] = queue.Queue()
        self.out: list[bytes] = []
        self.closed = threading.Event()
        self.worker = Worker(
            adapter or ScriptedAdapter(),
            JOB,
            job_dir,
            self.lines.get,
            self.out.append,
            self.closed.set,
        )
        self.exit_code: int | None = None
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self) -> None:
        self.exit_code = self.worker.run()

    def send(self, message: dict[str, Any]) -> None:
        message = {"v": 1, "jobId": JOB, **message}
        self.lines.put(json.dumps(message).encode() + b"\n")

    def send_raw(self, line: bytes) -> None:
        self.lines.put(line)

    def close(self) -> None:
        self.lines.put(b"")

    def messages(self) -> list[Any]:
        return [parse_worker_message(line) for line in list(self.out)]

    def wait_for(self, type_: str, timeout: float = 5) -> Any:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for m in self.messages():
                if m.type == type_:
                    return m
            time.sleep(0.01)
        raise AssertionError(f"no {type_} message; got {[m.type for m in self.messages()]}")

    def finish(self, timeout: float = 5) -> int | None:
        self.close()
        self.thread.join(timeout)
        assert not self.thread.is_alive(), "worker did not stop"
        return self.exit_code


@pytest.fixture
def job_dir(tmp_path: Path) -> Path:
    for d in ("in", "out", "work"):
        (tmp_path / d).mkdir()
    (tmp_path / "in" / "0.pdf").write_bytes(b"%PDF-1.7 input")
    return tmp_path


def execute(params: dict[str, Any], secrets: dict[str, Any] | None = None) -> dict[str, Any]:
    msg: dict[str, Any] = {
        "type": "execute",
        "inputs": ["in/0.pdf"],
        "outDir": "out",
        "workDir": "work",
        "params": params,
    }
    if secrets:
        msg["secrets"] = secrets
    return msg


def test_hello_comes_first(job_dir: Path) -> None:
    h = Harness(job_dir)
    hello = h.wait_for("hello")
    assert h.messages()[0] is not None and h.messages()[0].type == "hello"
    assert hello.adapterId == "scripted"
    assert hello.protocolVersion == 1
    assert hello.jobId == JOB
    assert h.finish() == 0


def test_validate_and_estimate(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send({"type": "validate", "inputs": ["in/0.pdf"], "params": {"x": 7}})
    r = h.wait_for("validateResult")
    assert r.metadata.root == {"inputs": 1, "seen": 7}
    h.send({"type": "estimate", "inputs": ["in/0.pdf"], "params": {}, "metadata": {"n": 4}})
    e = h.wait_for("estimateResult")
    assert (e.ramBytes, e.diskBytes, e.costUnits) == (10, 20, 4)
    h.finish()


def test_execute_reports_hashed_outputs(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send(execute({}))
    r = h.wait_for("executeResult")
    [out] = r.outputs
    assert out.path == "out/0.pdf"
    assert out.suggestedName == "Report (final).pdf"
    data = (job_dir / "out" / "0.pdf").read_bytes()
    assert out.sizeBytes == len(data)
    assert out.sha256 == hashlib.sha256(data).hexdigest()
    assert r.fidelity == "E"
    types = [m.type for m in h.messages()]
    assert types.index("stage") < types.index("executeResult")
    h.finish()


def test_adapter_error_is_structured(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send(execute({"mode": "fail"}))
    e = h.wait_for("error")
    assert e.code == "E_PASSWORD_WRONG"
    assert e.retryable is True
    assert e.detail.root == {"page": 3}
    h.finish()


def test_crash_is_internal_and_leaks_nothing(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send(execute({"mode": "crash"}, {"password": SECRET}))
    e = h.wait_for("error")
    assert e.code == "E_INTERNAL"
    assert e.detail.root == {"exception": "ValueError"}
    raw = b"".join(h.out)
    assert SECRET.encode() not in raw
    assert b"document text" not in raw
    h.finish()


@pytest.mark.parametrize("mode", ["escape", "unreserved", "none"])
def test_bad_outputs_are_rejected(job_dir: Path, mode: str) -> None:
    h = Harness(job_dir)
    h.send(execute({"mode": mode}))
    assert h.wait_for("error").code == "E_INTERNAL"
    assert not any(m.type == "executeResult" for m in h.messages())
    h.finish()


def test_cancel_interrupts_execute(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send(execute({"mode": "slow"}))
    h.wait_for("progress")
    started = time.monotonic()
    h.send({"type": "cancel"})
    e = h.wait_for("error")
    assert e.code == "E_CANCELLED"
    assert e.retryable is False
    assert time.monotonic() - started < 1
    h.finish()


def test_stdin_close_stops_running_job(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send(execute({"mode": "slow"}))
    h.wait_for("progress")
    started = time.monotonic()
    assert h.finish(timeout=1) == 0
    assert time.monotonic() - started < 1
    assert h.closed.is_set()


def test_progress_is_throttled(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send(execute({"mode": "flood"}))
    h.wait_for("executeResult")
    progress = [m for m in h.messages() if m.type == "progress"]
    assert len(progress) < 50
    h.finish()


@pytest.mark.parametrize(
    ("line", "reason"),
    [
        (b"not json\n", "schema"),
        (b'{"v":1,"type":"execute","jobId":"j_other","inputs":["in/0.pdf"],"params":{}}\n', None),
        (b'{"v":1,"type":"inspect","jobId":"j_test","input":"../escape.pdf"}\n', "schema"),
        (b"x" * (1024 * 1024 + 10) + b"\n", "lineTooLong"),
    ],
)
def test_protocol_violations_end_the_worker(job_dir: Path, line: bytes, reason: str | None) -> None:
    h = Harness(job_dir)
    h.send_raw(line)
    e = h.wait_for("error")
    assert e.code == "E_PROTOCOL"
    if reason:
        assert e.detail.root == {"reason": reason}
    h.thread.join(2)
    assert h.exit_code == 2


def test_unsupported_method(job_dir: Path) -> None:
    h = Harness(job_dir)
    h.send({"type": "render", "input": "in/0.pdf", "page": 1, "dpi": 96, "out": "out/p1.png"})
    e = h.wait_for("error")
    assert e.code == "E_INTERNAL"
    assert e.detail.root["reason"] == "notSupported"
    h.finish()


@pytest.mark.parametrize(
    ("raw", "safe"),
    [
        ("report.pdf", "report.pdf"),
        ("../../etc/passwd", "_.._etc_passwd"),
        ("C:\\Windows\\x.pdf", "C__Windows_x.pdf"),
        ("a\x00b\nc.pdf", "a_b_c.pdf"),
        ("...", "output"),
        ("", "output"),
        ("x" * 300 + ".pdf", "x" * 251 + ".pdf"),
    ],
)
def test_safe_display_name(raw: str, safe: str) -> None:
    assert safe_display_name(raw) == safe


def test_workers_never_read_the_system_mime_tables(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The sandbox makes these files unreadable; reading one would raise.
    from converter_sdk import runtime  # noqa: PLC0415

    unreadable = tmp_path / "mime.types"
    unreadable.write_text("application/x-local  local\n")
    unreadable.chmod(0)
    monkeypatch.setattr(mimetypes, "knownfiles", [str(unreadable)])
    monkeypatch.setattr(mimetypes, "inited", False)
    monkeypatch.setattr(mimetypes, "_db", None)
    runtime._builtin_mime_types()
    assert mimetypes.guess_type("a.local") == (None, None)
    assert mimetypes.guess_type("a.png")[0] == "image/png"
