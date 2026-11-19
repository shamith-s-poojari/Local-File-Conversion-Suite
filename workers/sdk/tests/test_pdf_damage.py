"""PY-2: a garbled content stream in the input is the input's fault."""

from __future__ import annotations

from pathlib import Path

import pikepdf
import pytest
from converter_conformance.harness import context, make_job_dir
from converter_sdk import AdapterError
from converter_sdk.pdf import save_pdf


def _pdf_with_stream(path: Path, data: bytes, *, flate: bool = True) -> Path:
    pdf = pikepdf.new()
    pdf.add_blank_page(page_size=(200, 200))
    stream = pikepdf.Stream(pdf, data)
    if flate:
        stream.stream_dict["/Filter"] = pikepdf.Name.FlateDecode
    pdf.pages[0].obj.Contents = pdf.make_indirect(stream)
    pdf.save(path)
    return path


def _save(tmp_path: Path, src: Path) -> None:
    job_dir, rel = make_job_dir(tmp_path / "job", [src])
    ctx, _ = context(job_dir, rel)
    with ctx.open_pdf(ctx.inputs[0]) as pdf:
        save_pdf(ctx, pdf, ctx.output_path("o.pdf"))


@pytest.mark.parametrize("data", [b"x\x9c\x01garbagegarbage", b"\x00\x01 not flate at all"])
def test_garbled_flate_content_stream_is_input_corrupt(tmp_path: Path, data: bytes) -> None:
    src = _pdf_with_stream(tmp_path / "bad.pdf", data)
    with pytest.raises(AdapterError) as caught:
        _save(tmp_path, src)
    assert (caught.value.code, caught.value.retryable) == ("E_INPUT_CORRUPT", False)


def test_truncated_flate_content_stream_is_input_corrupt(tmp_path: Path) -> None:
    import zlib  # noqa: PLC0415

    good = zlib.compress(b"BT /F1 12 Tf 10 10 Td (hello hello hello) Tj ET " * 50)
    src = _pdf_with_stream(tmp_path / "cut.pdf", good[: len(good) // 2])
    with pytest.raises(AdapterError) as caught:
        _save(tmp_path, src)
    assert caught.value.code == "E_INPUT_CORRUPT"


def test_an_undamaged_input_still_saves(tmp_path: Path) -> None:
    src = _pdf_with_stream(tmp_path / "ok.pdf", b"0 0 10 10 re f", flate=False)
    _save(tmp_path, src)
