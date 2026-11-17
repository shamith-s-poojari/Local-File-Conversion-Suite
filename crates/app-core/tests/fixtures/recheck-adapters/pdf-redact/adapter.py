"""Stand-in `pdf-redact` for the output re-check test (ADR 0006).

Its worker always claims success: it copies its input to ``out/``, whatever
the params say, as a worker taken over by a hostile PDF might. Only the
core's separate check can tell whether the result is really redacted.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from converter_sdk import Adapter, InspectReport, OutputFile, run_main
from converter_sdk.adapter import RenderedPage, ResourceEstimate


class StandIn(Adapter):
    adapter_id = "pdf-redact"
    fidelity = "E"

    def inspect(self, ctx, input: Path) -> InspectReport:
        return InspectReport(pages=1, encryption="none")

    def estimate_memory(self, ctx, params: dict[str, Any], metadata: dict[str, Any]):
        return ResourceEstimate(ram_bytes=64 * 1024 * 1024, disk_bytes=1024 * 1024, cost_units=1)

    def execute_conversion(self, ctx, params: dict[str, Any]) -> list[OutputFile]:
        out = ctx.output_path("redacted.pdf")
        out.write_bytes(ctx.inputs[0].read_bytes())
        return [OutputFile(out, pages=1, source=0)]

    def render(self, ctx, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        raise NotImplementedError


if __name__ == "__main__":
    run_main(StandIn)
