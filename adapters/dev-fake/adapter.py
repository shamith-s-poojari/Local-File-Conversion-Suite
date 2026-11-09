"""dev-fake: a stand-in adapter for the walking skeleton and supervisor tests.

It copies the first input to ``out/`` unchanged while reporting simulated
per-page progress. Parameters (see ``params.schema.json``) make it warn,
fail, crash, hang or print to stdout on purpose. It parses nothing: page
counts and encryption come from byte patterns, which is fine for fixtures
and wrong for real files.
"""

from __future__ import annotations

import re
import struct
import time
import zlib
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    AdapterError,
    InspectReport,
    JobContext,
    OutputFile,
    PageWords,
    RenderedPage,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.protocol import ValidationIssue, WordBox

SCAN_LIMIT = 16 * 1024 * 1024
COPY_CHUNK = 256 * 1024
_PAGE = re.compile(rb"/Type\s*/Page(?![A-Za-z])")


def _head(path: Path) -> bytes:
    with path.open("rb") as f:
        return f.read(SCAN_LIMIT)


def _check_pdf(ctx: JobContext, path: Path) -> bytes:
    """Fake 'open': corrupt if there's no PDF header; password rules from /Encrypt."""
    data = _head(path)
    if not data.startswith(b"%PDF-"):
        raise AdapterError("E_INPUT_CORRUPT")
    if b"/Encrypt" in data:
        password = ctx.secrets.password if ctx.secrets is not None else None
        if password is None:
            raise AdapterError("E_PASSWORD_REQUIRED")
        if password.get_secret_value() != "fake-password":
            raise AdapterError("E_PASSWORD_WRONG")
    return data


def _png(width: int, height: int) -> bytes:
    """A blank 8-bit grayscale PNG."""

    def chunk(kind: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
        )

    row = b"\x00" + b"\xff" * width
    raw = zlib.compress(row * height, 1)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")


class DevFakeAdapter(Adapter):
    adapter_id: ClassVar[str] = "dev-fake"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        data = _head(input)
        if not data.startswith(b"%PDF-"):
            raise AdapterError("E_INPUT_CORRUPT")
        encryption: Literal["none", "openPassword"] = (
            "openPassword" if b"/Encrypt" in data else "none"
        )
        return InspectReport(pages=len(_PAGE.findall(data)), encryption=encryption)

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        report = ValidationReport()
        total = 0
        for path in ctx.inputs:
            relative = path.relative_to(ctx.job_dir.resolve()).as_posix()
            if not path.is_file():
                report.errors.append(ValidationIssue(code="E_INPUT_CORRUPT", input=relative))
                continue
            total += path.stat().st_size
            try:
                _check_pdf(ctx, path)
            except AdapterError as e:
                report.errors.append(ValidationIssue(code=e.code, input=relative))
        report.metadata = {"inputBytes": total, "inputs": len(ctx.inputs)}
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        input_bytes = int(metadata.get("inputBytes", 0))
        return ResourceEstimate(
            ram_bytes=32 * 1024 * 1024 + COPY_CHUNK,
            disk_bytes=input_bytes,
            cost_units=int(params.get("pages", 5)),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        pages = int(params.get("pages", 5))
        delay = int(params.get("delayMs", 20)) / 1000
        source = ctx.inputs[0]
        _check_pdf(ctx, source)

        ctx.stage("process", 1, 2)
        for page in range(1, pages + 1):
            if params.get("hang"):
                while True:  # Deliberately ignores cancel; only the watchdog can stop this.
                    time.sleep(1)
            ctx.check_cancelled()
            time.sleep(delay)
            if params.get("noise"):
                print(f"stray library output on page {page}")  # noqa: T201
            if page == 1 and "warn" in params:
                ctx.warn(params["warn"], {"page": page})
            if page == 1 and "failWith" in params:
                raise AdapterError(params["failWith"], {"page": page})
            if page == 1 and params.get("crash"):
                raise RuntimeError("simulated crash with document text that must not leak")
            ctx.progress("process", page, pages)

        ctx.stage("write", 2, 2)
        out = ctx.output_path("fake-output.pdf")
        size = source.stat().st_size
        done = 0
        with source.open("rb") as src, out.open("wb") as dst:
            while chunk := src.read(COPY_CHUNK):
                ctx.check_cancelled()
                dst.write(chunk)
                done += len(chunk)
                ctx.progress("write", done, size)
        return [OutputFile(out, pages=pages)]

    def render(self, ctx: JobContext, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        _check_pdf(ctx, input)
        # US Letter at the requested DPI.
        width, height = round(8.5 * dpi), round(11 * dpi)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(_png(width, height))
        return RenderedPage(width_px=width, height_px=height)

    def page_text(self, ctx: JobContext, input: Path, page: int) -> PageWords:
        _check_pdf(ctx, input)
        return PageWords(
            width=612.0,
            height=792.0,
            words=[WordBox(text="fake", x0=72.0, y0=72.0, x1=100.0, y1=84.0)],
        )

    def cleanup(self, ctx: JobContext) -> None:
        for path in ctx.work_dir.glob("tmp*"):
            path.unlink(missing_ok=True)


if __name__ == "__main__":
    run_main(DevFakeAdapter)
