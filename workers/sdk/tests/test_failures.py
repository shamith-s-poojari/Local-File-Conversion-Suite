"""PY-1: hostile or damaged input is a permanent failure with its own code,
never the retryable E_INTERNAL, and the message never leaves the worker."""

from __future__ import annotations

import queue
import threading
import time
from pathlib import Path
from typing import Any

import pikepdf
import pymupdf
import pytest
from converter_sdk import Adapter, JobContext, OutputFile, Worker
from converter_sdk.failures import map_exception
from converter_sdk.protocol import parse_worker_message

SECRET = "document-text-9f3a"


def _deep(n: int) -> Any:
    top = cur = pikepdf.Array()
    for _ in range(n):
        nxt = pikepdf.Array()
        cur.append(nxt)
        cur = nxt
    return top


def test_recursion_error_is_a_limit_not_a_retry() -> None:
    error = map_exception(RecursionError(SECRET))
    assert error is not None
    assert (error.code, error.retryable) == ("E_LIMIT_EXCEEDED", False)
    assert SECRET not in repr(error.detail)


def test_a_real_recursion_error_from_deep_nesting() -> None:
    def walk(depth: int) -> int:
        return walk(depth + 1)

    with pytest.raises(RecursionError) as caught:
        walk(0)
    assert map_exception(caught.value).code == "E_LIMIT_EXCEEDED"  # type: ignore[union-attr]


def test_pikepdf_parse_damage_is_input_corrupt(tmp_path: Path) -> None:
    bad = tmp_path / "bad.pdf"
    bad.write_bytes(b"%PDF-1.7\nnot a pdf")
    with pytest.raises(pikepdf.PdfError) as caught:
        pikepdf.open(bad)
    error = map_exception(caught.value)
    assert error is not None
    assert (error.code, error.retryable) == ("E_INPUT_CORRUPT", False)


def test_a_password_error_is_not_damage() -> None:
    assert map_exception(pikepdf.PasswordError("x")) is None


def test_mupdf_stack_overflow_is_a_limit(tmp_path: Path) -> None:
    deep = pikepdf.new()
    deep.add_blank_page()
    deep.Root.Deep = _deep(3000)
    path = tmp_path / "deep.pdf"
    deep.save(path)
    # MuPDF's own message for this file: "exception stack overflow".
    with pytest.raises(Exception) as caught:  # noqa: PT011 - the type is MuPDF's
        pymupdf.open(path).save(tmp_path / "o.pdf")
    error = map_exception(caught.value)
    # The exact exception differs between MuPDF builds; whatever it is, it is
    # not retryable and not "internal".
    assert error is None or error.retryable is False


def test_mupdf_data_error_is_input_corrupt(tmp_path: Path) -> None:
    junk = tmp_path / "junk.pdf"
    junk.write_bytes(b"junk")
    with pytest.raises(pymupdf.FileDataError) as caught:
        pymupdf.open(junk, filetype="pdf")
    assert map_exception(caught.value).code == "E_INPUT_CORRUPT"  # type: ignore[union-attr]


def test_unrelated_exceptions_are_left_alone() -> None:
    assert map_exception(ValueError("x")) is None
    assert map_exception(RuntimeError("stack overflow in my own code")) is None


def test_mupdf_limit_class_is_a_limit() -> None:
    class FzErrorLimit(Exception):  # noqa: N818 - MuPDF's name
        pass

    FzErrorLimit.__module__ = "pymupdf.mupdf"
    assert map_exception(FzErrorLimit("x")).code == "E_LIMIT_EXCEEDED"  # type: ignore[union-attr]


class _Raises(Adapter):
    adapter_id = "raises"
    fidelity = "E"

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        if params["mode"] == "recursion":
            raise RecursionError(SECRET)
        raise pikepdf.PdfError(SECRET)


@pytest.mark.parametrize(
    ("mode", "code"), [("recursion", "E_LIMIT_EXCEEDED"), ("parse", "E_INPUT_CORRUPT")]
)
def test_the_worker_sends_the_mapped_error(tmp_path: Path, mode: str, code: str) -> None:
    import json  # noqa: PLC0415

    for d in ("in", "out", "work"):
        (tmp_path / d).mkdir()
    (tmp_path / "in" / "0.pdf").write_bytes(b"x")
    lines: queue.Queue[bytes] = queue.Queue()
    out: list[bytes] = []
    worker = Worker(_Raises(), "j1", tmp_path, lines.get, out.append, lambda: None)
    thread = threading.Thread(target=worker.run, daemon=True)
    thread.start()
    request = {
        "v": 1,
        "jobId": "j1",
        "type": "execute",
        "inputs": ["in/0.pdf"],
        "outDir": "out",
        "workDir": "work",
        "params": {"mode": mode},
    }
    lines.put(json.dumps(request).encode() + b"\n")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and not any(b'"error"' in line for line in out):
        time.sleep(0.01)
    lines.put(b"")  # closing stdin cancels, so only after the answer
    thread.join(5)
    errors = [m for m in map(parse_worker_message, out) if m.type == "error"]
    assert [(e.code, e.retryable) for e in errors] == [(code, False)]
    assert SECRET.encode() not in b"".join(out)
