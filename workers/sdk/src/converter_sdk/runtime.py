"""Worker runtime: the protocol loop around an adapter (spec sections 7 and 8.3).

One worker process serves one job. The core starts it with the job directory
as its working directory and the job ID as its only argument, for example::

    python -I adapters/pdf-merge/adapter.py j_7f3a

The worker sends ``hello``, then answers requests one at a time until stdin
closes. ``cancel`` is read concurrently so it can interrupt a running request.
"""

from __future__ import annotations

import errno
import hashlib
import mimetypes
import os
import queue
import re
import sys
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any

from pydantic import ValidationError

from . import protocol
from .adapter import Adapter
from .context import JobContext, OutputFile, resolve_job_path
from .errors import AdapterError, Cancelled
from .protocol import (
    Cancel,
    Estimate,
    EstimateResult,
    Execute,
    ExecuteOutput,
    ExecuteResult,
    Hello,
    Inspect,
    InspectResult,
    PageText,
    PageTextResult,
    Render,
    RenderResult,
    Validate,
    ValidateResult,
    WorkerError,
)

JOB_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")
STDIN_CLOSED_GRACE_S = 0.8
"""Spec 7.1: exit within 1 s of stdin closing. The process is ended after this."""

EXIT_OK = 0
EXIT_PROTOCOL = 2
EXIT_USAGE = 64

_EOF = object()
_HASH_CHUNK = 1024 * 1024


class _ProtocolViolation(Exception):
    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class Worker:
    """The protocol loop, independent of process-level setup so it can be tested in-process."""

    def __init__(
        self,
        adapter: Adapter,
        job_id: str,
        job_dir: Path,
        read_line: Callable[[], bytes],
        write: Callable[[bytes], None],
        on_stdin_closed: Callable[[], None] | None = None,
    ) -> None:
        self.adapter = adapter
        self.job_id = job_id
        self.job_dir = job_dir
        self._read_line = read_line
        self._write = write
        self._on_stdin_closed = on_stdin_closed
        self._write_lock = threading.Lock()
        self._cancel = threading.Event()
        self._requests: queue.Queue[object] = queue.Queue()

    # ---------------------------------------------------------------- output

    def send(self, message: protocol.WorkerMessage) -> None:
        line = protocol.encode(message)
        with self._write_lock:
            self._write(line)

    def _error(self, code: str, detail: dict[str, Any] | None, retryable: bool) -> None:
        self.send(WorkerError(jobId=self.job_id, code=code, detail=detail, retryable=retryable))

    # ----------------------------------------------------------------- input

    def _reader(self) -> None:
        try:
            while True:
                line = self._read_line()
                if not line:
                    break
                if line.strip() == b"":
                    continue
                try:
                    message = protocol.parse_core_message(line)
                except protocol.LineTooLongError:
                    self._requests.put(_ProtocolViolation("lineTooLong"))
                    return
                except ValidationError:
                    self._requests.put(_ProtocolViolation("schema"))
                    return
                if message.jobId != self.job_id:
                    self._requests.put(_ProtocolViolation("jobIdMismatch"))
                    return
                if isinstance(message, Cancel):
                    self._cancel.set()
                else:
                    self._requests.put(message)
        except OSError:
            pass
        # stdin closed: stop whatever is running and leave.
        self._cancel.set()
        self._requests.put(_EOF)
        if self._on_stdin_closed is not None:
            self._on_stdin_closed()

    # ------------------------------------------------------------------ loop

    def run(self) -> int:
        self.send(
            Hello(
                v=protocol.PROTOCOL_VERSION,
                jobId=self.job_id,
                adapterId=self.adapter.adapter_id,
                protocolVersion=protocol.PROTOCOL_VERSION,
                pid=os.getpid(),
            )
        )
        threading.Thread(target=self._reader, name="protocol-reader", daemon=True).start()
        while True:
            request = self._requests.get()
            if request is _EOF:
                return EXIT_OK
            if isinstance(request, _ProtocolViolation):
                self._error("E_PROTOCOL", {"reason": request.reason}, False)
                return EXIT_PROTOCOL
            self._handle(request)

    def _context(self, inputs: list[str], secrets: protocol.Secrets | None) -> JobContext:
        return JobContext(
            job_id=self.job_id,
            job_dir=self.job_dir,
            inputs=[resolve_job_path(self.job_dir, p) for p in inputs],
            secrets=secrets,
            _send=self.send,
            _cancel=self._cancel,
        )

    def _handle(self, request: object) -> None:
        ctx: JobContext | None = None
        try:
            match request:
                case Inspect():
                    ctx = self._context([request.input], request.secrets)
                    self.send(self._inspect(ctx))
                case Validate():
                    ctx = self._context([p.root for p in request.inputs], request.secrets)
                    self.send(self._validate(ctx, request))
                case Estimate():
                    ctx = self._context([p.root for p in request.inputs], None)
                    self.send(self._estimate(ctx, request))
                case Execute():
                    ctx = self._context([p.root for p in request.inputs], request.secrets)
                    try:
                        self.send(self._execute(ctx, request))
                    finally:
                        self._cleanup(ctx)
                case Render():
                    ctx = self._context([request.input], request.secrets)
                    self.send(self._render(ctx, request))
                case PageText():
                    ctx = self._context([request.input], request.secrets)
                    self.send(self._page_text(ctx, request))
                case _:
                    raise AdapterError("E_INTERNAL", {"reason": "unhandledMessage"})
        except AdapterError as e:
            self._error(e.code, e.detail, e.retryable)
        except ValidationError:
            # The adapter returned values the schema rejects.
            self._error("E_INTERNAL", {"reason": "invalidResult"}, True)
        except MemoryError:
            self._error("E_RESOURCE_MEMORY", None, True)
        except OSError as e:
            if e.errno == errno.ENOSPC:
                self._error("E_RESOURCE_DISK", None, True)
            else:
                # Only the type and errno: messages can contain file paths.
                self._error("E_INTERNAL", {"exception": type(e).__name__, "errno": e.errno}, True)
        except Exception as e:
            # Never the message: it may contain document text.
            self._error("E_INTERNAL", {"exception": type(e).__name__}, True)

    # ------------------------------------------------------------- handlers

    def _inspect(self, ctx: JobContext) -> InspectResult:
        r = self.adapter.inspect(ctx, ctx.inputs[0])
        return InspectResult(
            jobId=self.job_id,
            pages=r.pages,
            encryption=r.encryption,
            signed=r.signed,
            warnings=r.warnings or None,
            media=r.media,
        )

    def _validate(self, ctx: JobContext, request: Validate) -> ValidateResult:
        r = self.adapter.validate_input(ctx, request.params.model_dump())
        return ValidateResult(
            jobId=self.job_id, errors=r.errors, warnings=r.warnings, metadata=r.metadata
        )

    def _estimate(self, ctx: JobContext, request: Estimate) -> EstimateResult:
        r = self.adapter.estimate_memory(ctx, request.params.model_dump(), request.metadata.root)
        return EstimateResult(
            jobId=self.job_id,
            ramBytes=r.ram_bytes,
            diskBytes=r.disk_bytes,
            costUnits=r.cost_units,
        )

    def _execute(self, ctx: JobContext, request: Execute) -> ExecuteResult:
        ctx.check_cancelled()
        outputs = self.adapter.execute_conversion(ctx, request.params.model_dump())
        ctx.check_cancelled()
        if not outputs:
            raise AdapterError("E_INTERNAL", {"reason": "noOutputs"})
        return ExecuteResult(
            jobId=self.job_id,
            outputs=[self._describe_output(ctx, o) for o in outputs],
            fidelity=self.adapter.fidelity,
        )

    def _describe_output(self, ctx: JobContext, output: OutputFile) -> ExecuteOutput:
        name = ctx.suggested_name(output.path)
        path = output.path
        if path.is_symlink() or not path.is_file():
            raise AdapterError("E_INTERNAL", {"reason": "outputMissing"})
        relative = _job_relative(self.job_dir, path)
        if not relative.startswith("out/"):
            raise AdapterError("E_INTERNAL", {"reason": "outputOutsideOut"})
        size, digest = _size_and_sha256(path)
        return ExecuteOutput(
            path=relative, suggestedName=name, pages=output.pages, sizeBytes=size, sha256=digest
        )

    def _render(self, ctx: JobContext, request: Render) -> RenderResult:
        out = resolve_job_path(self.job_dir, request.out)
        r = self.adapter.render(ctx, ctx.inputs[0], request.page, request.dpi, out)
        if out.is_symlink() or not out.is_file():
            raise AdapterError("E_INTERNAL", {"reason": "outputMissing"})
        size, digest = _size_and_sha256(out)
        return RenderResult(
            jobId=self.job_id,
            out=request.out,
            widthPx=r.width_px,
            heightPx=r.height_px,
            sizeBytes=size,
            sha256=digest,
        )

    def _page_text(self, ctx: JobContext, request: PageText) -> PageTextResult:
        r = self.adapter.page_text(ctx, ctx.inputs[0], request.page)
        return PageTextResult(
            jobId=self.job_id, page=request.page, width=r.width, height=r.height, words=r.words
        )

    def _cleanup(self, ctx: JobContext) -> None:
        try:
            self.adapter.cleanup(ctx)
        except Exception:  # noqa: S110 - cleanup is best effort; the core deletes the job dir
            pass


def _job_relative(job_dir: Path, path: Path) -> str:
    try:
        return path.resolve().relative_to(job_dir.resolve()).as_posix()
    except ValueError:
        raise AdapterError("E_INTERNAL", {"reason": "outputOutsideJob"}) from None


def _size_and_sha256(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as f:
        while chunk := f.read(_HASH_CHUNK):
            digest.update(chunk)
            size += len(chunk)
    return size, digest.hexdigest()


# ----------------------------------------------------------- process setup


def _take_protocol_channel() -> tuple[Any, Any]:
    """Spec 7.1: move the protocol to private descriptors, point fd 1 at
    stderr and fd 0 at the null device, so stray ``print`` or ``input`` calls
    in a library can't corrupt or steal protocol traffic."""
    proto_out = os.fdopen(os.dup(1), "wb", buffering=0)
    proto_in = os.fdopen(os.dup(0), "rb")
    sys.stdout.flush()
    os.dup2(2, 1)
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    os.close(devnull)
    sys.stdout = sys.stderr
    sys.stdin = open(os.devnull, encoding="utf-8")  # noqa: SIM115 - lives for the process
    return proto_in, proto_out


def _builtin_mime_types() -> None:
    """Use Python's own MIME table, never the system's.

    Libraries such as openpyxl call ``mimetypes.init()`` on import, which
    reads ``/etc/mime.types`` and friends. Those files sit outside the
    sandbox, so the read fails; and they differ between machines, which
    would make outputs differ too. On Windows the registry plays the same
    part, so it's skipped as well.
    """
    mimetypes.knownfiles = []
    for name in ("_mimetypes_read_windows_registry", "_winreg"):
        if hasattr(mimetypes, name):
            setattr(mimetypes, name, None)
    mimetypes.init()


def run_main(adapter_cls: type[Adapter], argv: list[str] | None = None) -> None:
    """Entry point for an adapter script: ``if __name__ == "__main__": run_main(MyAdapter)``."""
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 1 or not JOB_ID.fullmatch(args[0]):
        sys.stderr.write("usage: adapter.py <jobId>\n")
        sys.exit(EXIT_USAGE)

    proto_in, proto_out = _take_protocol_channel()
    _builtin_mime_types()

    def read_line() -> bytes:
        # +2 leaves room for "\r\n"; anything longer is rejected by the parser.
        return proto_in.readline(protocol.MAX_LINE_BYTES + 2)

    def write(data: bytes) -> None:
        proto_out.write(data)

    def on_stdin_closed() -> None:
        timer = threading.Timer(STDIN_CLOSED_GRACE_S, os._exit, args=(EXIT_OK,))
        timer.daemon = True
        timer.start()

    worker = Worker(adapter_cls(), args[0], Path.cwd(), read_line, write, on_stdin_closed)
    try:
        code = worker.run()
    except Cancelled:
        code = EXIT_OK
    proto_out.flush()
    # Skip interpreter teardown: daemon threads and engine finalizers can stall it.
    os._exit(code)
