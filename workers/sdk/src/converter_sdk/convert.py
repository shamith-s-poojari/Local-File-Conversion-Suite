"""Shared behaviour for conversion adapters (ADR 0003).

A conversion adapter declares its edges once, in ``EDGES``; a test in each
adapter checks the manifest lists the same ones. The core stages inputs as
``in/<n>.<format>``, puts the target format in ``params["to"]`` and the
inputs' original names (without extension) in ``params["fileTitles"]``.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, ClassVar, Literal

from .adapter import Adapter, ValidationReport
from .context import JobContext
from .errors import AdapterError
from .protocol import ValidationIssue

Tier = Literal["E", "H", "R", "L"]
_RANK = {"E": 0, "H": 1, "R": 2, "L": 3}


def source_format(path: Path) -> str:
    """The sniffed format of a staged input, from its staged extension."""
    return path.suffix.lstrip(".").lower()


def issue(param: str, reason: str) -> ValidationIssue:
    return ValidationIssue(code="E_INTERNAL", param=param, detail={"reason": reason})


def worst(tiers: list[Tier]) -> Tier:
    return max(tiers, key=lambda t: _RANK[t]) if tiers else "E"


class ConversionAdapter(Adapter):
    """Base for adapters with ``kind: conversion``."""

    EDGES: ClassVar[dict[tuple[str, str], Tier]] = {}
    fidelity: ClassVar[Tier] = "L"  # replaced per job by the edges actually used

    # ----------------------------------------------------------- edge table

    @classmethod
    def targets_for(cls, source: str) -> list[str]:
        return [to for (frm, to) in cls.EDGES if frm == source]

    def target(self, params: dict[str, Any]) -> str:
        to = str(params.get("to", "")).lower()
        if not to:
            raise AdapterError("E_INTERNAL", {"reason": "noTarget"}, retryable=False)
        return to

    def tier(self, source: str, to: str) -> Tier:
        try:
            return self.EDGES[(source, to)]
        except KeyError:
            raise AdapterError(
                "E_INTERNAL", {"reason": "unsupportedConversion"}, retryable=False
            ) from None

    def use_tiers(self, ctx: JobContext, to: str) -> None:
        """Record this job's fidelity: the worst tier among its inputs."""
        self.fidelity = worst([self.tier(source_format(p), to) for p in ctx.inputs])  # type: ignore[misc]

    # ------------------------------------------------------------ validation

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        to = str(params.get("to", "")).lower()
        if not to:
            return ValidationReport(errors=[issue("to", "required")])
        for p in ctx.inputs:
            if (source_format(p), to) not in self.EDGES:
                return ValidationReport(errors=[issue("to", "unsupportedConversion")])
        return self.validate_sources(ctx, params, to)

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        """Per-adapter checks of each input (sizes, limits, encryption)."""
        return ValidationReport()

    # ----------------------------------------------------------------- names

    @staticmethod
    def title(params: dict[str, Any], index: int, fallback: str = "file") -> str:
        titles = params.get("fileTitles") or []
        if index < len(titles) and isinstance(titles[index], str) and titles[index].strip():
            return titles[index].strip()
        return f"{fallback}-{index + 1}"

    def output_name(self, params: dict[str, Any], index: int, ext: str, suffix: str = "") -> str:
        return f"{self.title(params, index)}{suffix}.{ext}"


def pdf_bytes_to_output(ctx: JobContext, data: bytes, path: Path) -> int:
    """Write PDF bytes made by another engine (PyMuPDF, LibreOffice) under the
    shared saving rules (spec 13.2): full rewrite, deterministic /ID, checked.
    Returns the page count."""
    import pikepdf  # noqa: PLC0415 - engine imported on demand

    from .pdf import save_pdf  # noqa: PLC0415 - pdf helpers import pikepdf

    scratch = ctx.temp_path(".pdf")
    scratch.write_bytes(data)
    try:
        with pikepdf.open(scratch) as pdf:
            # Engines stamp creation times; drop them so repeat runs match.
            with pdf.open_metadata(set_pikepdf_as_editor=False, update_docinfo=False) as meta:
                for key in ("xmp:CreateDate", "xmp:ModifyDate", "xmp:MetadataDate"):
                    if key in meta:
                        del meta[key]
            # The engine's random /ID (and LibreOffice's /DocChecksum of its
            # own, timestamped, output) would survive a deterministic save.
            for key in ("/ID", "/DocChecksum"):
                if key in pdf.trailer:
                    del pdf.trailer[key]
            if "/Info" in pdf.trailer:
                info = pdf.trailer.Info
                for key in ("/CreationDate", "/ModDate", "/Producer", "/Creator"):
                    if key in info:
                        del info[key]
            return save_pdf(ctx, pdf, path)
    except pikepdf.PdfError:
        raise AdapterError("E_OUTPUT_INVALID", {"reason": "engineOutput"}) from None
    finally:
        scratch.unlink(missing_ok=True)
