"""office-convert checks that never start LibreOffice (P3, P14, P6)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance import corpus
from converter_conformance.harness import context, load_adapter_class, make_job_dir
from converter_sdk import AdapterError, limits

ADAPTER = Path(__file__).resolve().parents[1]
CLS = load_adapter_class(ADAPTER)
CFB = bytes.fromhex("D0CF11E0A1B11AE1")


def test_password_protected_office_file_needs_a_password(tmp_path: Path) -> None:
    locked = tmp_path / "locked.docx"
    locked.write_bytes(CFB + b"\0" * 1024)
    job, rel = make_job_dir(tmp_path / "job", [locked])
    ctx, _ = context(job, rel)
    report = CLS().validate_input(ctx, {"to": "pdf"})
    assert [e.code for e in report.errors] == ["E_PASSWORD_REQUIRED"]


def test_pdf_pages_are_checked_before_they_are_rendered(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(limits, "MAX_PIXELS", 1_000_000)
    src = corpus.text_pdf(tmp_path / "a.pdf", 1)
    job, rel = make_job_dir(tmp_path / "job", [src])
    ctx, _ = context(job, rel)
    with pytest.raises(AdapterError) as e:
        CLS()._from_pdf(ctx, ctx.inputs[0], "png", "t", {"dpi": 300})
    assert e.value.code == "E_LIMIT_EXCEEDED"


def test_decrypted_copy_warns(tmp_path: Path) -> None:
    src = corpus.encrypted_pdf(tmp_path / "enc.pdf")
    job, rel = make_job_dir(tmp_path / "job", [src])
    ctx, sent = context(job, rel, {"password": corpus.SENTINEL_PASSWORD})
    CLS()._plain_pdf(ctx, ctx.inputs[0])
    assert [m.code for m in sent if getattr(m, "type", None) == "warning"] == ["ENCRYPTION_REMOVED"]
