"""Starting LibreOffice the way the sandbox must: soffice.bin, no launcher."""

from __future__ import annotations

import threading
from pathlib import Path
from typing import Any

import pytest
from converter_sdk import JobContext, office
from converter_sdk.errors import AdapterError


def make_ctx(tmp_path: Path) -> JobContext:
    (tmp_path / "work").mkdir()
    return JobContext(
        job_id="j_test",
        job_dir=tmp_path,
        inputs=[],
        secrets=None,
        _send=lambda _m: None,
        _cancel=threading.Event(),
    )


def fake_soffice(codes: list[int], calls: list[list[Any]]):
    """Exits with each of ``codes`` in turn; writes a PDF when it exits 0."""

    def run(_ctx: JobContext, argv: list[Any], **_kw: Any) -> tuple[int, str]:
        calls.append(argv)
        code = codes[len(calls) - 1]
        if code == 0:
            outdir = Path(argv[argv.index("--outdir") + 1])
            (outdir / "note.pdf").write_bytes(b"%PDF-1.7\n")
        return code, ""

    return run


@pytest.fixture
def soffice(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    monkeypatch.setenv("LFCS_BIN_SOFFICE", str(tmp_path / "soffice.bin"))

    def use(codes: list[int]) -> list[list[Any]]:
        calls: list[list[Any]] = []
        monkeypatch.setattr(office, "run", fake_soffice(codes, calls))
        return calls

    return use


def test_a_first_start_that_asks_for_a_restart_is_started_again(soffice, tmp_path: Path) -> None:
    # soffice.bin exits 81 after making a new profile; the launcher script
    # the sandbox can't run would have started it again.
    calls = soffice([office.RESTART_REQUESTED, 0])
    produced = office.soffice_convert(make_ctx(tmp_path), tmp_path / "note.html", "pdf")
    assert [p.name for p in produced] == ["note.pdf"]
    assert len(calls) == 2


def test_it_is_restarted_only_once(soffice, tmp_path: Path) -> None:
    calls = soffice([office.RESTART_REQUESTED, office.RESTART_REQUESTED])
    with pytest.raises(AdapterError) as e:
        office.soffice_convert(make_ctx(tmp_path), tmp_path / "note.html", "pdf")
    assert e.value.code == "E_INPUT_CORRUPT"
    assert len(calls) == 2


def test_other_failures_are_not_retried(soffice, tmp_path: Path) -> None:
    calls = soffice([1])
    with pytest.raises(AdapterError):
        office.soffice_convert(make_ctx(tmp_path), tmp_path / "note.html", "pdf")
    assert len(calls) == 1
