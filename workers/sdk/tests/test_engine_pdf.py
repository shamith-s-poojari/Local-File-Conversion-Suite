"""PDFs another engine made are saved so repeat runs give the same bytes."""

from __future__ import annotations

import io
import threading
import uuid
from pathlib import Path

import pikepdf
from converter_sdk import JobContext
from converter_sdk.convert import pdf_bytes_to_output


def make_ctx(tmp_path: Path) -> JobContext:
    for d in ("in", "out", "work"):
        (tmp_path / d).mkdir(parents=True, exist_ok=True)
    return JobContext(
        job_id="j_test",
        job_dir=tmp_path,
        inputs=[],
        secrets=None,
        _send=lambda _m: None,
        _cancel=threading.Event(),
    )


def engine_pdf() -> bytes:
    """A one-page PDF stamped the way newer LibreOffice stamps one: random
    XMP document and instance IDs and a creation date."""
    pdf = pikepdf.new()
    pdf.add_blank_page()
    with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:
        meta["dc:title"] = "Report"
        meta["xmpMM:DocumentID"] = f"uuid:{uuid.uuid4()}"
        meta["xmpMM:InstanceID"] = f"uuid:{uuid.uuid4()}"
        meta["xmp:CreateDate"] = "2026-11-18T08:00:00Z"
    buf = io.BytesIO()
    pdf.save(buf, static_id=False)
    return buf.getvalue()


def test_random_xmp_ids_dont_make_repeat_runs_differ(tmp_path: Path) -> None:
    outs = []
    for run in ("a", "b"):
        ctx = make_ctx(tmp_path / run)
        out = ctx.output_path("report.pdf")
        pdf_bytes_to_output(ctx, engine_pdf(), out)
        outs.append(out.read_bytes())
    assert outs[0] == outs[1]
    with pikepdf.open(io.BytesIO(outs[0])) as pdf, pdf.open_metadata() as meta:
        assert "xmpMM:DocumentID" not in meta
        assert "xmpMM:InstanceID" not in meta
        assert meta.get("dc:title") == "Report"
