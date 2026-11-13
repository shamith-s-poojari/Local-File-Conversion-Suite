"""External engine binaries (ADR 0003).

The core discovers FFmpeg and LibreOffice at startup and, for adapters that
list them in ``requires``, grants the sandbox access and passes each
binary's path in ``LFCS_BIN_<NAME>``. Adapters never search ``PATH``.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Sequence
from pathlib import Path

from .context import JobContext
from .errors import AdapterError, Cancelled


def binary(name: str) -> Path:
    """The granted path of an engine binary, e.g. ``binary("ffmpeg")``."""
    value = os.environ.get(f"LFCS_BIN_{name.upper().replace('-', '_')}")
    if not value:
        raise AdapterError("E_INSTALL_CORRUPT", {"missing": name}, retryable=False)
    path = Path(value)
    if not path.is_absolute():
        raise AdapterError("E_INSTALL_CORRUPT", {"missing": name}, retryable=False)
    return path


def available(name: str) -> bool:
    return bool(os.environ.get(f"LFCS_BIN_{name.upper().replace('-', '_')}"))


def run(
    ctx: JobContext,
    argv: Sequence[str | os.PathLike[str]],
    *,
    on_line: Callable[[str], None] | None = None,
    env: dict[str, str] | None = None,
    timeout_s: float | None = None,
    cwd: Path | None = None,
) -> tuple[int, str]:
    """Run an engine, reading stdout line by line (for progress) and keeping
    the tail of stderr (never shown to the user: it can contain file text).

    Cancellation kills the engine and everything it started at once: its
    process group on macOS and Linux, a Job Object on Windows.
    Returns the exit code and the last 8 KB of stderr.
    """
    full_env = dict(os.environ)
    if env:
        full_env.update(env)
    # A finished job's work/ is gone, but preview workers still run engines
    # there (it is also HOME and TMPDIR).
    ctx.work_dir.mkdir(exist_ok=True)
    proc = subprocess.Popen(  # noqa: S603 - argv comes from the adapter, not the user
        [os.fspath(a) for a in argv],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=full_env,
        cwd=cwd or ctx.work_dir,
        **_NEW_GROUP,
    )
    group = _Group(proc)
    stdout, stderr = proc.stdout, proc.stderr
    if stdout is None or stderr is None:  # both are pipes; never None
        group.kill()
        raise AdapterError("E_INTERNAL", {"reason": "enginePipes"}, retryable=False)
    tail: list[bytes] = []

    def drain_stderr() -> None:
        for chunk in iter(lambda: stderr.read(4096), b""):
            tail.append(chunk)
            while sum(len(c) for c in tail) > 8192 and len(tail) > 1:
                tail.pop(0)

    reader = threading.Thread(target=drain_stderr, daemon=True)
    reader.start()

    stop = threading.Event()
    timed_out = threading.Event()

    def watch() -> None:
        deadline = time.monotonic() + timeout_s if timeout_s else None
        while not stop.wait(0.1):
            late = deadline is not None and time.monotonic() > deadline
            if ctx.cancelled or late:
                if late:
                    timed_out.set()
                group.kill()
                return

    watcher = threading.Thread(target=watch, daemon=True)
    watcher.start()
    try:
        for raw in stdout:
            if on_line is not None:
                on_line(raw.decode("utf-8", "replace").rstrip("\r\n"))
        code = proc.wait()
    finally:
        stop.set()
        if proc.poll() is None:
            group.kill()
        group.close()
        reader.join(timeout=2)
    if ctx.cancelled:
        raise Cancelled()
    if timed_out.is_set():
        raise AdapterError("E_TIMEOUT", {"reason": "engine"})
    return code, b"".join(tail).decode("utf-8", "replace")


if sys.platform == "win32":
    _NEW_GROUP: dict = {
        "creationflags": subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP
    }
else:
    _NEW_GROUP = {"start_new_session": True}


class _Group:
    """The engine and everything it starts, killed together. A cancelled or
    timed-out job's output is deleted anyway, so nothing finishes cleanly.

    On Windows the engine goes into its own Job Object (nested in the
    worker's). A child it starts before that, in the moment after it
    starts, stays in the worker's job, which the runner kills when the
    worker exits."""

    def __init__(self, proc: subprocess.Popen[bytes]) -> None:
        self.proc = proc
        self.job = None
        if sys.platform == "win32":
            try:
                self.job = _WinJob(proc)
            except OSError:
                self.job = None

    def kill(self) -> None:
        if self.job is not None:
            self.job.kill()
        elif sys.platform == "win32":
            self.proc.kill()
        else:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                return
        try:
            self.proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            pass

    def close(self) -> None:
        if self.job is not None:
            self.job.close()
            self.job = None


class _WinJob:
    def __init__(self, proc: subprocess.Popen[bytes]) -> None:
        import ctypes  # noqa: PLC0415 - Windows only
        from ctypes import wintypes  # noqa: PLC0415

        k = ctypes.WinDLL("kernel32", use_last_error=True)  # type: ignore[attr-defined]
        k.CreateJobObjectW.restype = wintypes.HANDLE
        k.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        k.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        k.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
        k.CloseHandle.argtypes = [wintypes.HANDLE]
        self._k = k
        self._h = k.CreateJobObjectW(None, None)
        if not self._h:
            raise OSError(ctypes.get_last_error(), "CreateJobObjectW")  # type: ignore[attr-defined]
        if not k.AssignProcessToJobObject(self._h, int(proc._handle)):  # type: ignore[attr-defined]
            err = ctypes.get_last_error()  # type: ignore[attr-defined]
            k.CloseHandle(self._h)
            raise OSError(err, "AssignProcessToJobObject")

    def kill(self) -> None:
        self._k.TerminateJobObject(self._h, 1)

    def close(self) -> None:
        self._k.CloseHandle(self._h)
