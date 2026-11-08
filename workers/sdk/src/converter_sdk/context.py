"""What an adapter method gets to work with (spec section 8.3)."""

from __future__ import annotations

import re
import threading
import time
import unicodedata
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .errors import AdapterError, Cancelled
from .protocol import Progress, Secrets, Stage, WorkerWarning

if TYPE_CHECKING:
    from .protocol import WorkerMessage

PROGRESS_INTERVAL_S = 0.1
"""Progress is throttled to 10 Hz per stage; the core kills workers above 1,000 messages/s."""

_EXTENSION = re.compile(r"[a-z0-9]{1,8}")
_UNSAFE_NAME_CHARS = re.compile(r'[\x00-\x1f\x7f<>:"/\\|?*]')


def safe_display_name(name: str, fallback: str = "output") -> str:
    """Make a suggested file name harmless: no separators, control or reserved
    characters, no leading dots, at most 255 characters. The core sanitizes it
    again before use; this keeps the protocol message valid."""
    name = unicodedata.normalize("NFC", name)
    name = _UNSAFE_NAME_CHARS.sub("_", name).strip().lstrip(".").strip()
    if len(name) > 255:
        stem, dot, ext = name.rpartition(".")
        name = (stem[: 254 - len(ext)] + dot + ext) if dot and len(ext) <= 16 else name[:255]
    return name or fallback


@dataclass(frozen=True)
class OutputFile:
    """One output an adapter produced. ``path`` must come from ``ctx.output_path``."""

    path: Path
    pages: int | None = None


@dataclass
class JobContext:
    """Per-request context. The worker's working directory is the job directory."""

    job_id: str
    job_dir: Path
    inputs: list[Path]
    secrets: Secrets | None
    _send: Callable[[WorkerMessage], None]
    _cancel: threading.Event
    _outputs: dict[Path, str] = field(default_factory=dict)
    _last_progress: dict[str, float] = field(default_factory=dict)
    _temp_counter: int = 0

    @property
    def out_dir(self) -> Path:
        return self.job_dir / "out"

    @property
    def work_dir(self) -> Path:
        return self.job_dir / "work"

    # ------------------------------------------------------------ cancellation

    @property
    def cancelled(self) -> bool:
        return self._cancel.is_set()

    def check_cancelled(self) -> None:
        """Raise ``Cancelled`` if the core asked to stop. Call at least once per page."""
        if self._cancel.is_set():
            raise Cancelled()

    # ------------------------------------------------------------------ events

    def stage(self, name: str, index: int, of: int) -> None:
        self._send(Stage(jobId=self.job_id, name=name, index=index, of=of))

    def progress(self, stage: str, done: int, total: int | None = None) -> None:
        """Report progress. Throttled, except that completion is always sent."""
        now = time.monotonic()
        finished = total is not None and done >= total
        if not finished and now - self._last_progress.get(stage, -1e9) < PROGRESS_INTERVAL_S:
            return
        self._last_progress[stage] = now
        self._send(Progress(jobId=self.job_id, stage=stage, done=done, total=total))

    def warn(self, code: str, detail: dict[str, Any] | None = None) -> None:
        """Send a warning. Use catalog codes (spec section 13.1) listed in the manifest."""
        self._send(WorkerWarning(jobId=self.job_id, code=code, detail=detail))

    # ------------------------------------------------------------------- paths

    def temp_path(self, suffix: str = "") -> Path:
        """A fresh path under ``work/``. The core deletes the job directory afterwards."""
        if suffix and not re.fullmatch(r"\.[A-Za-z0-9]{1,8}", suffix):
            raise ValueError("suffix must look like '.ext'")
        self._temp_counter += 1
        self.work_dir.mkdir(exist_ok=True)
        return self.work_dir / f"tmp{self._temp_counter}{suffix}"

    def output_path(self, suggested_name: str) -> Path:
        """Reserve an output file under ``out/``. The on-disk name is generated
        (``out/<n>.<ext>``); ``suggested_name`` is only what the user is offered."""
        display = safe_display_name(suggested_name)
        ext = display.rpartition(".")[2].lower() if "." in display else ""
        ext = ext if _EXTENSION.fullmatch(ext) else "bin"
        self.out_dir.mkdir(exist_ok=True)
        path = self.out_dir / f"{len(self._outputs)}.{ext}"
        self._outputs[path] = display
        return path

    def input_path(self, job_path: str) -> Path:
        """Resolve a job path from a request, refusing anything outside the job directory."""
        return resolve_job_path(self.job_dir, job_path)

    def suggested_name(self, path: Path) -> str:
        try:
            return self._outputs[path]
        except KeyError:
            raise AdapterError("E_INTERNAL", {"reason": "outputNotReserved"}) from None

    # ---------------------------------------------------------------- engines

    def open_pdf(self, path: Path) -> Any:
        """Open a PDF with pikepdf, using the request's password if there is one.

        All PDF adapters must open inputs through this, so password handling
        and error mapping live in one place (spec section 8.3).
        """
        try:
            import pikepdf  # noqa: PLC0415 - optional engine, imported on demand
        except ImportError:
            raise AdapterError("E_INSTALL_CORRUPT", {"missing": "pikepdf"}) from None

        password = ""
        if self.secrets is not None and self.secrets.password is not None:
            password = self.secrets.password.get_secret_value()
        try:
            return pikepdf.open(path, password=password)
        except pikepdf.PasswordError:
            code = "E_PASSWORD_WRONG" if password else "E_PASSWORD_REQUIRED"
            raise AdapterError(code) from None
        except pikepdf.PdfError:
            raise AdapterError("E_INPUT_CORRUPT") from None
        finally:
            del password


def resolve_job_path(job_dir: Path, job_path: str) -> Path:
    """The schema already forbids absolute paths and ``..``; this is the second check."""
    root = job_dir.resolve()
    resolved = (root / job_path).resolve()
    if resolved != root and root not in resolved.parents:
        raise AdapterError("E_PROTOCOL", {"reason": "pathOutsideJob"})
    return resolved
