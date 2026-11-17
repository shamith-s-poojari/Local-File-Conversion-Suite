"""Test stand-in worker. The input's bytes choose the behaviour:

- ``SLOW``: inspect takes 8 s; a conversion runs until cancelled (60 s).
- ``CRASH``: the worker exits before it says hello (a start failure).
- ``BREAK``: the first render kills the worker mid-request; later ones work.
- anything else: fast.
"""

from __future__ import annotations

import os
import sys
import time
from pathlib import Path
from typing import Any

PNG = (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00"
    b"\x1f\x15\xc4\x89\x00\x00\x00\rIDATx\x9cc\x00\x01\x00\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00"
    b"\x00\x00IEND\xaeB`\x82"
)


def _marker() -> bytes:
    try:
        return Path("in/0.pdf").read_bytes()
    except OSError:
        return b""


if b"CRASH" in _marker():
    sys.exit(3)

from converter_sdk import Adapter, InspectReport, OutputFile, run_main  # noqa: E402
from converter_sdk.adapter import RenderedPage, ResourceEstimate  # noqa: E402


class StandIn(Adapter):
    adapter_id = "pdf-redact"
    fidelity = "E"

    def inspect(self, ctx, input: Path) -> InspectReport:
        if b"SLOW" in input.read_bytes():
            time.sleep(8)
        return InspectReport(pages=1, encryption="none")

    def estimate_memory(self, ctx, params: dict[str, Any], metadata: dict[str, Any]):
        return ResourceEstimate(ram_bytes=64 * 1024 * 1024, disk_bytes=1024 * 1024, cost_units=1)

    def execute_conversion(self, ctx, params: dict[str, Any]) -> list[OutputFile]:
        data = ctx.inputs[0].read_bytes()
        if b"SLOW" in data:
            for _ in range(600):
                ctx.check_cancelled()
                time.sleep(0.1)
        out = ctx.output_path("out.pdf")
        out.write_bytes(data)
        return [OutputFile(out, pages=1, source=0)]

    def render(self, ctx, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        if b"BREAK" in input.read_bytes() and not Path("out/crashed").exists():
            Path("out/crashed").write_bytes(b"")
            os._exit(1)
        out.write_bytes(PNG)
        return RenderedPage(width_px=1, height_px=1)


if __name__ == "__main__":
    run_main(StandIn)
