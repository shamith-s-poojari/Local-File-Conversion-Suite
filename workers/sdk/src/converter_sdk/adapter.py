"""The adapter contract (spec section 8.3). Subclass ``Adapter`` and override
the methods your tool supports; the SDK runtime handles the protocol."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, ClassVar, Literal

from .context import JobContext, OutputFile
from .errors import AdapterError
from .protocol import MediaInfo, ValidationIssue, WarningItem, WordBox


@dataclass
class InspectReport:
    pages: int
    encryption: Literal["none", "permissionsOnly", "openPassword"]
    signed: bool | None = None
    warnings: list[WarningItem] = field(default_factory=list)
    media: MediaInfo | None = None


@dataclass
class ValidationReport:
    errors: list[ValidationIssue] = field(default_factory=list)
    warnings: list[WarningItem] = field(default_factory=list)
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class ResourceEstimate:
    ram_bytes: int
    disk_bytes: int
    cost_units: int


@dataclass
class RenderedPage:
    width_px: int
    height_px: int


@dataclass
class PageWords:
    width: float
    height: float
    words: list[WordBox]


def _unsupported(method: str) -> AdapterError:
    return AdapterError("E_INTERNAL", {"reason": "notSupported", "method": method}, retryable=False)


class Adapter:
    """Base class for every adapter.

    Class attributes mirror the manifest; a test in each adapter checks they agree.
    """

    adapter_id: ClassVar[str]
    fidelity: ClassVar[Literal["E", "H", "R", "L"]]

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        """Read-only facts about one input."""
        raise _unsupported("inspect")

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        """Adapter-specific checks, within 30 s. Must not write anywhere."""
        return ValidationReport()

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        """Resource estimate, within 1 s, from metadata already gathered."""
        raise _unsupported("estimate")

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        """Do the work. Write only to ``ctx.out_dir`` and ``ctx.work_dir``, and
        call ``ctx.check_cancelled()`` at least once per page."""
        raise _unsupported("execute")

    def render(self, ctx: JobContext, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        """Render one page to a PNG at ``out``."""
        raise _unsupported("render")

    def page_text(self, ctx: JobContext, input: Path, page: int) -> PageWords:
        """Word boxes for one page."""
        raise _unsupported("pageText")

    def cleanup(self, ctx: JobContext) -> None:  # noqa: B027 - optional hook
        """Idempotent; tolerates a partial run. Never relied on for safety."""
