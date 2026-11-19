"""Shared limits, the raster guards built on them, and the other helpers the
2026-11-15 audit added to the SDK (P3, P6, P8, P11, P14, P18)."""

from __future__ import annotations

import re
import struct
import sys
import zlib
from pathlib import Path

import pytest
from converter_conformance import corpus
from converter_conformance.harness import context, make_job_dir
from converter_sdk import (
    AdapterError,
    adapter,
    convert,
    engines,
    images,
    limits,
    office,
    pdf,
    tables,
)
from PIL import Image

ROOT = Path(__file__).resolve().parents[3]


def _code(exc: pytest.ExceptionInfo[AdapterError]) -> tuple[str, str | None]:
    return exc.value.code, (exc.value.detail or {}).get("reason")


# ----------------------------------------------------------- check_pixels


def test_check_pixels_limits() -> None:
    limits.check_pixels(10_000, 10_000)  # 100 million: fine
    with pytest.raises(AdapterError) as e:
        limits.check_pixels(20_000, 10_001)
    assert _code(e) == ("E_LIMIT_EXCEEDED", "pixels")
    with pytest.raises(AdapterError) as e:
        limits.check_pixels(1, 1, frames=limits.MAX_FRAMES + 1)
    assert _code(e) == ("E_LIMIT_EXCEEDED", "frames")
    # frames × pixels, not just either on its own
    with pytest.raises(AdapterError) as e:
        limits.check_pixels(2000, 1000, frames=60)
    assert _code(e) == ("E_LIMIT_EXCEEDED", "framePixels")
    limits.check_pixels(2000, 1000, frames=40)
    with pytest.raises(AdapterError) as e:
        limits.check_pixels(-1, 5)
    assert e.value.code == "E_INPUT_CORRUPT"


def test_fit_scale_keeps_a_page_under_the_cap() -> None:
    assert limits.fit_scale(612, 792, 150 / 72) == 150 / 72  # fits: untouched
    scale = limits.fit_scale(14_400, 14_400, 300 / 72)
    w, h = limits.rendered_size(14_400, 14_400, scale)
    assert w * h <= limits.MAX_PIXELS
    assert scale < 300 / 72


# ----------------------------------------------------------------- images


def _png_header(width: int, height: int) -> bytes:
    """A PNG that declares ``width`` × ``height`` and holds no pixels: enough
    for ``Image.open``, which only reads the header."""

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b""))
        + chunk(b"IEND", b"")
    )


def test_max_pixels_is_the_real_limit(tmp_path: Path) -> None:
    """P11: Pillow's own limit (about 89 million) used to cut in first."""
    assert Image.MAX_IMAGE_PIXELS == limits.MAX_PIXELS == images.MAX_PIXELS
    inside = tmp_path / "inside.png"
    inside.write_bytes(_png_header(15_000, 10_000))  # 150 million
    img = images.open_image(inside)
    assert img.size == (15_000, 10_000)
    img.close()
    over = tmp_path / "over.png"
    over.write_bytes(_png_header(20_000, 10_001))
    with pytest.raises(AdapterError) as e:
        images.open_image(over)
    assert _code(e) == ("E_LIMIT_EXCEEDED", "pixels")


def _gif(path: Path, frames: int, size: tuple[int, int] = (1, 1)) -> Path:
    # Distinct neighbours: the GIF writer merges identical consecutive frames.
    shots = [Image.new("RGB", size, (n % 256, n // 256, 7)) for n in range(frames)]
    shots[0].save(path, save_all=True, append_images=shots[1:], duration=10)
    return path


def test_frame_count_is_capped(tmp_path: Path) -> None:
    """P8: a GIF of tiny frames is cheap to make and expensive to hold."""
    ok = _gif(tmp_path / "ok.gif", 20)
    images.open_image(ok).close()
    many = _gif(tmp_path / "many.gif", limits.MAX_FRAMES + 1)
    with pytest.raises(AdapterError) as e:
        images.open_image(many)
    assert _code(e) == ("E_LIMIT_EXCEEDED", "frames")


def test_frames_times_pixels_is_capped(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(limits, "MAX_ANIMATION_PIXELS", 1000)
    wide = _gif(tmp_path / "wide.gif", 20, (10, 10))  # 2,000 pixels in all
    with pytest.raises(AdapterError) as e:
        images.open_image(wide)
    assert _code(e) == ("E_LIMIT_EXCEEDED", "framePixels")
    images.open_image(_gif(tmp_path / "one.png", 1, (10, 10))).close()


def test_contact_sheet_over_the_cap_fails_before_allocating(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """P11: shrinking cells stops at a legible size; what is still too big fails."""
    from converter_sdk import sheets  # noqa: PLC0415

    monkeypatch.setattr(limits, "MAX_PIXELS", 1_000_000)
    monkeypatch.setattr(sheets, "MAX_PIXELS", 1_000_000)
    with pytest.raises(AdapterError) as e:
        sheets.contact_sheet([], columns=3, cell_px=100, min_size=(3000, 3000))
    assert e.value.code == "E_LIMIT_EXCEEDED"


# ------------------------------------------------------------ preview dpi


class _Preview(pdf.PdfAdapter):
    adapter_id = "test-preview"
    fidelity = "E"


def test_preview_dpi_is_clamped_to_the_cap(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """P3: a page can't be rendered larger than the cap, whatever dpi is asked."""
    monkeypatch.setattr(limits, "MAX_PIXELS", 250_000)
    src = corpus.text_pdf(tmp_path / "a.pdf", 1)
    job, rel = make_job_dir(tmp_path / "job", [src])
    ctx, _ = context(job, rel)
    out = tmp_path / "out.png"
    shown = _Preview().render(ctx, ctx.inputs[0], 1, 600, out)  # 600 dpi: ~35 million px
    assert shown.width_px * shown.height_px <= 250_000
    with Image.open(out) as png:
        assert png.size == (shown.width_px, shown.height_px)


# ------------------------------------------------------ ENCRYPTION_REMOVED


def _warnings(sent: list) -> list[str]:
    return [m.code for m in sent if getattr(m, "type", None) == "warning"]


def test_save_pdf_warns_once_when_encryption_is_dropped(tmp_path: Path) -> None:
    """P6: every PDF written without encryption from an encrypted input says so."""
    src = corpus.encrypted_pdf(tmp_path / "enc.pdf")
    job, rel = make_job_dir(tmp_path / "job", [src])
    ctx, sent = context(job, rel, {"password": corpus.SENTINEL_PASSWORD})
    with ctx.open_pdf(ctx.inputs[0]) as doc:
        pdf.save_pdf(ctx, doc, ctx.output_path("a.pdf"))
        pdf.save_pdf(ctx, doc, ctx.output_path("b.pdf"))
    assert _warnings(sent) == ["ENCRYPTION_REMOVED"]


def test_permissions_only_input_also_warns(tmp_path: Path) -> None:
    src = corpus.encrypted_pdf(tmp_path / "perm.pdf", user="")
    job, rel = make_job_dir(tmp_path / "job", [src])
    ctx, sent = context(job, rel)
    with ctx.open_pdf(ctx.inputs[0]) as doc:
        pdf.save_pdf(ctx, doc, ctx.output_path("a.pdf"))
    assert _warnings(sent) == ["ENCRYPTION_REMOVED"]


def test_no_warning_without_encryption_or_when_it_is_kept(tmp_path: Path) -> None:
    plain = corpus.text_pdf(tmp_path / "plain.pdf", 1)
    job, rel = make_job_dir(tmp_path / "job", [plain])
    ctx, sent = context(job, rel)
    with ctx.open_pdf(ctx.inputs[0]) as doc:
        pdf.save_pdf(ctx, doc, ctx.output_path("a.pdf"))
    assert _warnings(sent) == []
    import pikepdf  # noqa: PLC0415

    enc = pikepdf.Encryption(owner="o", user="u", R=6)
    with ctx.open_pdf(ctx.inputs[0]) as doc:
        pdf.save_pdf(ctx, doc, ctx.output_path("b.pdf"), encryption=enc, check_password="u")
    assert _warnings(sent) == []


def test_warn_if_encrypted_for_non_pdf_outputs(tmp_path: Path) -> None:
    enc = corpus.encrypted_pdf(tmp_path / "enc.pdf")
    plain = corpus.text_pdf(tmp_path / "plain.pdf", 1)
    job, rel = make_job_dir(tmp_path / "job", [enc, plain])
    ctx, sent = context(job, rel)
    pdf.warn_if_encrypted(ctx, ctx.inputs[1])
    assert _warnings(sent) == []
    pdf.warn_if_encrypted(ctx, ctx.inputs[0])
    pdf.warn_if_encrypted(ctx, ctx.inputs[0])
    assert _warnings(sent) == ["ENCRYPTION_REMOVED"]


def test_active_content_warning_uses_the_shared_detector(tmp_path: Path) -> None:
    """P7: one detector for the warning and the findings list. A launch action
    on a link was invisible to the old root-only check."""
    import pikepdf  # noqa: PLC0415
    from converter_sdk import active  # noqa: PLC0415
    from pikepdf import Array, Dictionary, Name, String  # noqa: PLC0415

    doc = pikepdf.new()
    doc.add_blank_page()
    assert not pdf.has_active_content(doc)
    link = doc.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Link,
            Rect=Array([0, 0, 10, 10]),
            A=Dictionary(S=Name.Launch, F=String("calc.exe")),
        )
    )
    doc.pages[0].Annots = Array([link])
    assert active.pdf_active_findings(doc)
    assert pdf.has_active_content(doc)
    doc.close()


# ------------------------------------------------------------ P14 Office


CFB = bytes.fromhex("D0CF11E0A1B11AE1")


def test_password_protected_ooxml_is_not_corrupt(tmp_path: Path) -> None:
    wrapped = tmp_path / "locked.docx"
    wrapped.write_bytes(CFB + b"\0" * 512)
    assert office.is_encrypted_ooxml(wrapped, "docx")
    with pytest.raises(AdapterError) as e:
        office.reject_encrypted_ooxml(wrapped, "docx")
    assert e.value.code == "E_PASSWORD_REQUIRED"
    assert not e.value.retryable  # a password can't be given to LibreOffice
    # A legacy .doc is a Compound File by design; a real zip is fine.
    assert not office.is_encrypted_ooxml(wrapped, "doc")
    real = tmp_path / "real.docx"
    real.write_bytes(b"PK\x03\x04" + b"\0" * 100)
    assert not office.is_encrypted_ooxml(real, "docx")


def test_encrypted_xlsx_gives_password_required_for_table_tools(tmp_path: Path) -> None:
    wrapped = tmp_path / "0.xlsx"
    wrapped.write_bytes(CFB + b"\0" * 512)
    with pytest.raises(AdapterError) as e:
        tables.source_size(wrapped)
    assert e.value.code == "E_PASSWORD_REQUIRED"


# ------------------------------------------------------------ P18 engines


def test_engines_get_a_minimal_environment(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LFCS_TEST_SECRET", "sentinel-value")
    monkeypatch.setenv("SOME_API_TOKEN", "sentinel-token")
    monkeypatch.setenv("HOME", str(tmp_path))
    job, rel = make_job_dir(tmp_path / "job", [])
    ctx, _ = context(job, rel)
    seen: list[str] = []
    code, _err = engines.run(
        ctx,
        [sys.executable, "-I", "-c", "import os; print(sorted(os.environ))"],
        on_line=seen.append,
        env={"EXTRA": "1"},
    )
    assert code == 0
    names = eval(seen[0])  # noqa: S307 - our own child's printed list
    assert "LFCS_TEST_SECRET" not in names and "SOME_API_TOKEN" not in names
    assert "EXTRA" in names and "HOME" in names  # the sandbox's work folder, and the caller's own
    # Variables an ordinary login shell sets stay behind too.
    assert not {"SHELL", "USER", "LOGNAME", "PYTHONPATH"} & set(names)
    assert engines.engine_env().get("PATH") in (None, "/usr/bin:/bin")


# ------------------------------------------------------- de-duplication


def test_one_issue_function() -> None:
    made = adapter.issue("speed", "invalid", limit=3)
    assert (made.code, made.param) == ("E_INTERNAL", "speed")
    assert made.detail.root == {"reason": "invalid", "limit": 3}
    assert convert.issue is adapter.issue and pdf.param_issue is adapter.issue


def test_mb_is_defined_once() -> None:
    """Adapters and SDK modules import ``MB`` instead of redefining it."""
    paths = list((ROOT / "adapters").glob("*/adapter.py"))
    paths += [
        p
        for p in (ROOT / "workers" / "sdk" / "src" / "converter_sdk").glob("*.py")
        if p.name != "limits.py"
    ]
    defining = [p.name for p in paths if re.search(r"^MB\s*=", p.read_text("utf-8"), re.M)]
    assert defining == []
    assert limits.MB == 1024 * 1024
