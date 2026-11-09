"""End-to-end tests: dev-fake runs as a real worker process, as the core would start it.

These cover the process-level rules in spec section 7.1 that in-process tests
can't: hello within 10 s, stray stdout can't corrupt the protocol, cancel
within 2 s (section 8.4), and exit within 1 s of stdin closing.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

import pytest
import yaml
from converter_sdk.protocol import parse_worker_message

ADAPTER_DIR = Path(__file__).resolve().parents[1]
ADAPTER = ADAPTER_DIR / "adapter.py"
JOB = "j_fake1"
SECRET = "sentinel-password-7c1e"

PDF = b"%PDF-1.7\n1 0 obj << /Type /Page >> endobj\n2 0 obj << /Type /Page >> endobj\n%%EOF\n"
ENCRYPTED_PDF = b"%PDF-1.7\ntrailer << /Encrypt 5 0 R >>\n%%EOF\n"


class Proc:
    """A worker process plus a reader thread collecting its protocol lines."""

    def __init__(self, job_dir: Path) -> None:
        self.p = subprocess.Popen(  # noqa: S603 - fixed argv, our own adapter
            [sys.executable, "-I", str(ADAPTER), JOB],
            cwd=job_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.lines: list[bytes] = []
        self.stderr = b""
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._read_err, daemon=True).start()

    def _read(self) -> None:
        assert self.p.stdout is not None
        for line in self.p.stdout:
            self.lines.append(line)

    def _read_err(self) -> None:
        assert self.p.stderr is not None
        self.stderr = self.p.stderr.read()

    def send(self, message: dict[str, Any]) -> None:
        assert self.p.stdin is not None
        self.p.stdin.write(json.dumps({"v": 1, "jobId": JOB, **message}).encode() + b"\n")
        self.p.stdin.flush()

    def messages(self) -> list[Any]:
        # Every stdout line must parse: that's the "stray print" guarantee.
        return [parse_worker_message(line) for line in list(self.lines)]

    def wait_for(self, type_: str, timeout: float = 10) -> Any:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for m in self.messages():
                if m.type == type_:
                    return m
            time.sleep(0.02)
        raise AssertionError(f"no {type_}; got {[m.type for m in self.messages()]}")

    def close_stdin(self) -> None:
        assert self.p.stdin is not None
        self.p.stdin.close()

    def kill(self) -> None:
        if self.p.poll() is None:
            self.p.kill()
        self.p.wait(5)


@pytest.fixture
def job_dir(tmp_path: Path) -> Path:
    for d in ("in", "out", "work"):
        (tmp_path / d).mkdir()
    (tmp_path / "in" / "0.pdf").write_bytes(PDF)
    (tmp_path / "in" / "1.pdf").write_bytes(ENCRYPTED_PDF)
    (tmp_path / "in" / "2.pdf").write_bytes(b"not a pdf at all")
    return tmp_path


@pytest.fixture
def proc(job_dir: Path):
    p = Proc(job_dir)
    yield p
    p.kill()


def execute(params: dict[str, Any], inputs: list[str] | None = None, **extra: Any) -> dict:
    return {
        "type": "execute",
        "inputs": inputs or ["in/0.pdf"],
        "outDir": "out",
        "workDir": "work",
        "params": params,
        **extra,
    }


def test_hello_within_10_seconds(proc: Proc) -> None:
    hello = proc.wait_for("hello", timeout=10)
    assert hello.adapterId == "dev-fake"
    assert hello.jobId == JOB
    # A Windows venv's python.exe is a launcher that runs the real interpreter
    # as a child, so the PIDs differ there. The packaged runtime has no launcher.
    if sys.platform != "win32":
        assert hello.pid == proc.p.pid


def test_full_job(proc: Proc, job_dir: Path) -> None:
    proc.send({"type": "inspect", "input": "in/0.pdf"})
    inspect = proc.wait_for("inspectResult")
    assert (inspect.pages, inspect.encryption) == (2, "none")

    proc.send({"type": "validate", "inputs": ["in/0.pdf"], "params": {}})
    validate = proc.wait_for("validateResult")
    assert validate.errors == []
    metadata = validate.metadata.root

    proc.send({"type": "estimate", "inputs": ["in/0.pdf"], "params": {}, "metadata": metadata})
    assert proc.wait_for("estimateResult").diskBytes == len(PDF)

    proc.send(execute({"pages": 3, "delayMs": 1}))
    result = proc.wait_for("executeResult")
    [out] = result.outputs
    data = (job_dir / out.path).read_bytes()
    assert data == PDF
    assert out.sha256 == hashlib.sha256(data).hexdigest()
    assert out.suggestedName == "fake-output.pdf"
    stages = [m.name for m in proc.messages() if m.type == "stage"]
    assert stages == ["process", "write"]


def test_stray_print_does_not_corrupt_protocol(proc: Proc) -> None:
    proc.send(execute({"pages": 3, "delayMs": 1, "noise": True}))
    proc.wait_for("executeResult")
    assert all(line.startswith(b"{") for line in proc.lines)
    proc.close_stdin()
    proc.p.wait(5)
    assert b"stray library output" in proc.stderr


def test_cancel_within_2_seconds(proc: Proc, job_dir: Path) -> None:
    proc.send(execute({"pages": 1000, "delayMs": 50}))
    proc.wait_for("progress")
    started = time.monotonic()
    proc.send({"type": "cancel"})
    assert proc.wait_for("error").code == "E_CANCELLED"
    assert time.monotonic() - started < 2


def test_exits_within_1_second_of_stdin_closing(proc: Proc) -> None:
    proc.send(execute({"pages": 1000, "delayMs": 50}))
    proc.wait_for("progress")
    started = time.monotonic()
    proc.close_stdin()
    proc.p.wait(1.5)
    assert time.monotonic() - started < 1.0 + 0.3  # process teardown slack


def test_hang_ignores_cancel_but_not_stdin_close(proc: Proc) -> None:
    """The hung-adapter fixture for the supervisor's watchdog (spec 8.4)."""
    proc.send(execute({"hang": True}))
    proc.wait_for("stage")
    proc.send({"type": "cancel"})
    time.sleep(0.5)
    assert proc.p.poll() is None
    assert not any(m.type == "error" for m in proc.messages())
    proc.close_stdin()
    proc.p.wait(1.5)


@pytest.mark.parametrize(
    ("inputs", "secrets", "code"),
    [
        (["in/1.pdf"], None, "E_PASSWORD_REQUIRED"),
        (["in/1.pdf"], {"password": "wrong"}, "E_PASSWORD_WRONG"),
        (["in/2.pdf"], None, "E_INPUT_CORRUPT"),
    ],
)
def test_structured_input_errors(proc: Proc, inputs, secrets, code) -> None:
    extra = {"secrets": secrets} if secrets else {}
    proc.send(execute({}, inputs, **extra))
    assert proc.wait_for("error").code == code


def test_correct_password_works(proc: Proc) -> None:
    proc.send(
        execute({"pages": 1, "delayMs": 0}, ["in/1.pdf"], secrets={"password": "fake-password"})
    )
    proc.wait_for("executeResult")


def test_secret_never_leaves_the_worker(proc: Proc, job_dir: Path) -> None:
    proc.send(execute({"pages": 1, "crash": True}, secrets={"password": SECRET}))
    assert proc.wait_for("error").code == "E_INTERNAL"
    proc.close_stdin()
    proc.p.wait(5)
    assert SECRET.encode() not in b"".join(proc.lines)
    assert SECRET.encode() not in proc.stderr
    for path in job_dir.rglob("*"):
        if path.is_file():
            assert SECRET.encode() not in path.read_bytes()


def test_warning_and_failure_hooks(proc: Proc) -> None:
    proc.send(
        execute({"pages": 2, "delayMs": 0, "warn": "TAGS_DROPPED", "failWith": "E_LIMIT_EXCEEDED"})
    )
    assert proc.wait_for("warning").code == "TAGS_DROPPED"
    error = proc.wait_for("error")
    assert (error.code, error.retryable) == ("E_LIMIT_EXCEEDED", False)


def test_render_and_page_text(proc: Proc, job_dir: Path) -> None:
    proc.send({"type": "render", "input": "in/0.pdf", "page": 1, "dpi": 36, "out": "out/p1.png"})
    r = proc.wait_for("renderResult")
    png = (job_dir / "out" / "p1.png").read_bytes()
    assert png.startswith(b"\x89PNG\r\n\x1a\n")
    assert (r.widthPx, r.heightPx, r.sizeBytes) == (306, 396, len(png))
    proc.send({"type": "pageText", "input": "in/0.pdf", "page": 1})
    assert proc.wait_for("pageTextResult").words[0].text == "fake"


def test_bad_argv_is_refused() -> None:
    p = subprocess.run(  # noqa: S603
        [sys.executable, "-I", str(ADAPTER), "../not-a-job-id"],
        capture_output=True,
        timeout=30,
    )
    assert p.returncode == 64
    assert p.stdout == b""


def test_manifest_matches_adapter() -> None:
    manifest = yaml.safe_load((ADAPTER_DIR / "manifest.yaml").read_text("utf-8"))
    spec = importlib.util.spec_from_file_location("dev_fake_adapter", ADAPTER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert manifest["id"] == module.DevFakeAdapter.adapter_id == ADAPTER_DIR.name
    assert manifest["fidelity"] == module.DevFakeAdapter.fidelity
    assert manifest["devOnly"] is True
    params = json.loads((ADAPTER_DIR / manifest["params"]).read_text("utf-8"))
    assert params["type"] == "object"
