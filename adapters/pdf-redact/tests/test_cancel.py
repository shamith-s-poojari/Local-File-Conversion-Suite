"""PY-3: the long per-page and per-object loops of pdf-redact stop on cancel."""

from __future__ import annotations

import sys
from pathlib import Path

import pikepdf
import pymupdf
import pytest
from converter_conformance.harness import context, load_adapter_class
from converter_sdk.errors import Cancelled

ADAPTER = Path(__file__).resolve().parents[1]


@pytest.fixture
def module():
    return sys.modules[load_adapter_class(ADAPTER).__module__]


def _cancelled_ctx(tmp_path: Path):
    ctx, _ = context(tmp_path, [])
    ctx._cancel.set()
    return ctx


def test_strip_with_pikepdf_checks_for_cancel(module, tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    with pytest.raises(Cancelled):
        module.strip_with_pikepdf(pdf, ["secret"], _cancelled_ctx(tmp_path))


def test_strip_with_mupdf_checks_for_cancel(module, tmp_path: Path) -> None:
    doc = pymupdf.open()
    doc.new_page()
    try:
        with pytest.raises(Cancelled):
            module.strip_with_mupdf(doc, ["secret"], _cancelled_ctx(tmp_path))
    finally:
        doc.close()


def test_rasterize_checks_for_cancel(module, tmp_path: Path) -> None:
    doc = pymupdf.open()
    doc.new_page()
    try:
        with pytest.raises(Cancelled):
            module.rasterize(doc, {0}, _cancelled_ctx(tmp_path))
    finally:
        doc.close()
