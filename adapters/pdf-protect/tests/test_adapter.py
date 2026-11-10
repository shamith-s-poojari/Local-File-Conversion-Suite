"""Acceptance tests for pdf-protect (spec section 13.6)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
import pypdfium2 as pdfium
import pytest
from converter_conformance import corpus
from converter_conformance.harness import run_in_process

ADAPTER = Path(__file__).resolve().parents[1]
PASSWORD = "Pässwörd-日本-✓"


def run(tmp_path: Path, src: Path, params: dict, secrets: dict | None = None):
    return run_in_process(ADAPTER, tmp_path / "job", [src], params, secrets)


@pytest.fixture
def three(tmp_path: Path) -> Path:
    return corpus.build("text-3", tmp_path / "corpus")


def test_protect_round_trip_keeps_content(tmp_path, three) -> None:
    protected = run(tmp_path / "p", three, {"mode": "protect"}, {"userPassword": PASSWORD})
    assert protected.names == ["protected.pdf"]
    assert corpus.page_texts(protected.outputs[0], PASSWORD) == corpus.page_texts(three)
    unlocked = run(tmp_path / "u", protected.outputs[0], {"mode": "unlock"}, {"password": PASSWORD})
    assert corpus.page_texts(unlocked.outputs[0]) == corpus.page_texts(three)
    with pikepdf.open(unlocked.outputs[0]) as pdf:
        assert not pdf.is_encrypted


def test_encryption_method_and_permissions(tmp_path, three) -> None:
    params = {"mode": "protect", "permissions": {"copy": False, "print": False}}
    out = run(tmp_path, three, params, {"userPassword": PASSWORD}).outputs[0]
    with pikepdf.open(out, password=PASSWORD) as pdf:
        assert pdf.encryption.R == 6
        assert "aesv3" in str(pdf.encryption.stream_method)  # AES-256
        assert pdf.allow.extract is False
        assert pdf.allow.print_highres is False
        assert pdf.allow.modify_other is True
        assert pdf.allow.accessibility is True
    with pytest.raises(pikepdf.PasswordError):
        pikepdf.open(out)


def test_second_engine_opens_non_ascii_password(tmp_path, three) -> None:
    out = run(tmp_path, three, {"mode": "protect"}, {"userPassword": PASSWORD}).outputs[0]
    doc = pdfium.PdfDocument(out, password=PASSWORD)
    assert len(doc) == 3
    doc.close()
    with pytest.raises(pdfium.PdfiumError):
        pdfium.PdfDocument(out, password="wrong")


def test_wrong_password_is_reported(tmp_path) -> None:
    src = corpus.build("encrypted", tmp_path / "corpus")
    r = run(tmp_path, src, {"mode": "unlock"}, {"password": "wrong"})
    assert [e.code for e in r.errors] == ["E_PASSWORD_WRONG"]


def test_unlock_permissions_only_needs_confirmation(tmp_path) -> None:
    src = corpus.build("permissions-only", tmp_path / "corpus")
    refused = run(tmp_path / "a", src, {"mode": "unlock"})
    assert [e.param for e in refused.errors] == ["confirmRights"]
    done = run(tmp_path / "b", src, {"mode": "unlock", "confirmRights": True})
    with pikepdf.open(done.outputs[0]) as pdf:
        assert not pdf.is_encrypted


def test_unlock_with_owner_password_needs_no_confirmation(tmp_path) -> None:
    src = corpus.build("permissions-only", tmp_path / "corpus")
    r = run(tmp_path, src, {"mode": "unlock"}, {"password": "owner-pw"})
    assert r.errors == [] and len(r.outputs) == 1


@pytest.mark.parametrize(
    ("corpus_name", "params", "param", "reason"),
    [
        ("permissions-only", {"mode": "protect"}, "mode", "alreadyEncrypted"),
        ("text-1", {"mode": "unlock"}, "mode", "notEncrypted"),
        ("text-1", {"mode": "protect"}, "userPassword", "required"),
        ("text-1", {"mode": "protect", "userPassword": "x"}, "secrets", "passwordInParams"),
    ],
)
def test_preconditions(tmp_path, corpus_name, params, param, reason) -> None:
    src = corpus.build(corpus_name, tmp_path / "corpus")
    secrets = {"userPassword": "x"} if reason != "required" else None
    r = run(tmp_path, src, params, secrets)
    assert [(e.param, e.detail.root["reason"]) for e in r.errors] == [(param, reason)]


def test_sentinel_password_stays_out_of_the_job_directory(tmp_path, three) -> None:
    r = run(tmp_path, three, {"mode": "protect"}, {"userPassword": corpus.SENTINEL_PASSWORD})
    sentinel = corpus.SENTINEL_PASSWORD.encode("utf-8")
    for path in (tmp_path / "job").rglob("*"):
        if path.is_file():
            assert sentinel not in path.read_bytes()
    assert sentinel not in repr(r.sent).encode("utf-8")
