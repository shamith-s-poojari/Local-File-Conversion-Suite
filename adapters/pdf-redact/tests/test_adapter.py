"""Acceptance tests for pdf-redact (spec section 13.8).

Each test checks that secrets can't be recovered **independently of the
verifier**: with pdfium's text, MuPDF's text, and the decoded bytes of
every stream and string. Every fixture also has text that must survive,
which catches boxes that land in the wrong place.
"""

from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path

import pikepdf
import pymupdf
import pypdfium2 as pdfium
import pytest
from converter_conformance.harness import (
    context,
    load_adapter_class,
    make_job_dir,
    run_in_process,
)
from converter_sdk import AdapterError

ADAPTER = Path(__file__).resolve().parents[1]
VERIFY = ADAPTER / "verify.py"

# pytest's importlib mode keeps conftest off sys.path; load its constants by path.
_spec = importlib.util.spec_from_file_location(
    "pdf_redact_fixtures", Path(__file__).with_name("conftest.py")
)
assert _spec is not None and _spec.loader is not None
_fixtures = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_fixtures)
KEEP, SECRET, SECRET2 = _fixtures.KEEP, _fixtures.SECRET, _fixtures.SECRET2


# ---------------------------------------------------------------- helpers


def text_pdfium(path: Path) -> str:
    doc = pdfium.PdfDocument(path)
    out = []
    for page in doc:
        tp = page.get_textpage()
        out.append(tp.get_text_bounded())
        tp.close()
    doc.close()
    return "\n".join(out)


def text_mupdf(path: Path) -> str:
    with pymupdf.open(path) as doc:
        return "\n".join(p.get_text() for p in doc)


def decoded_bytes(path: Path) -> bytes:
    with pikepdf.open(path) as pdf:
        buf = io.BytesIO()
        pdf.save(
            buf,
            stream_decode_level=pikepdf.StreamDecodeLevel.all,
            object_stream_mode=pikepdf.ObjectStreamMode.disable,
            compress_streams=False,
        )
    return buf.getvalue()


def recoverable(path: Path, secrets: list[str]) -> list[str]:
    """Every way a secret could still be found. Empty means it's gone."""
    found = []
    flat = lambda s: "".join(s.split()).casefold()  # noqa: E731
    texts = {"pdfium": text_pdfium(path), "mupdf": text_mupdf(path)}
    raw = decoded_bytes(path).lower()
    for secret in secrets:
        for engine, text in texts.items():
            if flat(secret) in flat(text.replace("-\n", "")):
                found.append(f"{secret} in {engine} text")
        for enc in ("latin-1", "utf-8", "utf-16-be"):
            data = secret.casefold().encode(enc)
            if data in raw or data.hex().encode() in raw:
                found.append(f"{secret} in raw {enc}")
    return found


def displayed_box(path: Path, needle: str, page: int = 0) -> dict:
    """Where ``needle`` appears as the user sees the page, normalized."""
    with pymupdf.open(path) as doc:
        p = doc[page]
        rect = p.search_for(needle)[0]
        d = (rect * p.rotation_matrix).normalize()
        w, h = p.rect.width, p.rect.height
    pad = 2
    return {
        "page": page + 1,
        "x0": max(0, (d.x0 - pad) / w),
        "y0": max(0, (d.y0 - pad) / h),
        "x1": min(1, (d.x1 + pad) / w),
        "y1": min(1, (d.y1 + pad) / h),
    }


def redact(tmp_path: Path, src: Path, **params):
    run = run_in_process(ADAPTER, tmp_path / "job", [src], params)
    assert run.errors == [], run.errors
    assert len(run.outputs) == 1
    return run


# ------------------------------------------------------------ basic removal


def test_terms_remove_every_occurrence(tmp_path, fixture) -> None:
    run = redact(tmp_path, fixture("plain"), terms=[SECRET.upper()])
    out = run.outputs[0]
    assert recoverable(out, [SECRET]) == []
    assert KEEP in text_pdfium(out)
    assert run.names == ["redacted.pdf"]


def test_box_removes_only_what_it_covers_and_warns_about_the_rest(tmp_path, fixture) -> None:
    src = fixture("plain")
    run = redact(tmp_path, src, boxes=[displayed_box(src, SECRET)])
    text = text_pdfium(run.outputs[0])
    assert "Account" in text and KEEP in text
    assert text.count(SECRET) == 1  # the second mention was not boxed
    warnings = [m for m in run.sent if getattr(m, "type", None) == "warning"]
    remains = [w for w in warnings if w.code == "REDACTED_TEXT_REMAINS_ELSEWHERE"]
    assert remains and remains[0].detail.root["count"] == 1


@pytest.mark.parametrize("name", ["rot-0", "rot-90", "rot-180", "rot-270", "rot-90-crop", "crop"])
def test_boxes_land_on_the_right_content(tmp_path, fixture, name) -> None:
    src = fixture(name)
    run = redact(tmp_path, src, boxes=[displayed_box(src, SECRET)])
    out = run.outputs[0]
    assert recoverable(out, [SECRET]) == []
    assert KEEP in text_pdfium(out), "the box hit the wrong text"


def test_text_split_across_operators_and_lines(tmp_path, fixture) -> None:
    run = redact(tmp_path, fixture("split"), terms=[SECRET, SECRET2])
    assert recoverable(run.outputs[0], [SECRET, SECRET2]) == []
    assert KEEP in text_pdfium(run.outputs[0])


def test_invisible_text_and_text_under_an_image(tmp_path, fixture) -> None:
    run = redact(tmp_path, fixture("hidden"), terms=[SECRET, SECRET2])
    assert recoverable(run.outputs[0], [SECRET, SECRET2]) == []
    assert KEEP in text_pdfium(run.outputs[0])


def test_text_inside_a_form_xobject(tmp_path, fixture) -> None:
    run = redact(tmp_path, fixture("xobject"), terms=[SECRET])
    assert recoverable(run.outputs[0], [SECRET]) == []
    assert KEEP in text_pdfium(run.outputs[0])


def test_image_pixels_under_a_box_are_changed(tmp_path, fixture) -> None:
    src = fixture("image")
    # The image covers x 100..300, y 100..300 (top-left origin) on a 612x792 page.
    box = {"page": 1, "x0": 150 / 612, "y0": 150 / 792, "x1": 250 / 612, "y1": 250 / 792}
    out = redact(tmp_path, src, boxes=[box]).outputs[0]
    doc = pdfium.PdfDocument(out)
    image = doc[0].render(scale=1).to_pil().convert("RGB")
    doc.close()
    assert image.getpixel((200, 200)) == (0, 0, 0)  # inside the box
    r, g, b = image.getpixel((120, 120))  # image, outside the box
    assert r > 180 and g < 80 and b < 80
    with pymupdf.open(out) as d:
        info = d[0].get_images(full=True)
        pix = pymupdf.Pixmap(d, info[0][0])
        assert pix.pixel(100, 100) != (220, 30, 30)  # the image data itself changed
        assert pix.pixel(10, 10) == (220, 30, 30)  # outside the box it didn't


def test_image_pixels_are_blanked_in_the_right_place_on_a_rotated_cropped_page(
    tmp_path, fixture
) -> None:
    """MuPDF misplaces *drawings* on such pages; its pixel blanking must still
    hit the area under the box, or a fill would hide unredacted pixels."""
    src = fixture("image-rot-crop")
    with pymupdf.open(src) as doc:
        page = doc[0]
        # The image's top-left quarter, in displayed coordinates.
        image_rect = page.get_image_rects(page.get_images()[0][0])[0]
        quarter = pymupdf.Rect(
            image_rect.x0,
            image_rect.y0,
            (image_rect.x0 + image_rect.x1) / 2,
            (image_rect.y0 + image_rect.y1) / 2,
        )
        d = (quarter * page.rotation_matrix).normalize()
        w, h = page.rect.width, page.rect.height
    box = {"page": 1, "x0": d.x0 / w, "y0": d.y0 / h, "x1": d.x1 / w, "y1": d.y1 / h}
    out = redact(tmp_path, src, boxes=[box]).outputs[0]
    with pymupdf.open(out) as doc:
        pix = pymupdf.Pixmap(doc, doc[0].get_images(full=True)[0][0])
        # Image space: top-left quarter is blanked, bottom-right is untouched.
        assert pix.pixel(pix.width // 4, pix.height // 4) != (220, 30, 30)
        assert pix.pixel(3 * pix.width // 4, 3 * pix.height // 4) == (220, 30, 30)


def test_document_level_secrets_are_stripped(tmp_path, fixture) -> None:
    run = redact(tmp_path, fixture("document"), terms=[SECRET, SECRET2])
    out = run.outputs[0]
    assert recoverable(out, [SECRET, SECRET2]) == []
    assert KEEP in text_pdfium(out)
    assert "FORMS_FLATTENED" in run.warnings
    with pikepdf.open(out) as pdf:
        assert set(pdf.docinfo.keys()) == {"/Producer"}
        for key in ("/Metadata", "/AcroForm", "/OCProperties", "/OpenAction"):
            assert key not in pdf.Root
        assert len(pdf.attachments) == 0
        for page in pdf.pages:
            assert "/Annots" not in page.obj and "/Thumb" not in page.obj
    with pymupdf.open(out) as doc:
        assert [t[1] for t in doc.get_toc()] == ["Overview"]


def test_history_from_incremental_saves_is_gone(tmp_path, fixture) -> None:
    src = fixture("incremental")
    assert src.read_bytes().count(b"startxref") == 2  # the fixture really has history
    out = redact(tmp_path, src, terms=["unrelated"]).outputs[0]
    data = out.read_bytes()
    assert data.count(b"startxref") == 1 and data.count(b"%%EOF") == 1
    assert recoverable(out, [SECRET]) == []


def test_scanned_page_box_works_and_search_warns(tmp_path, fixture) -> None:
    box = {"page": 1, "x0": 0.1, "y0": 0.1, "x1": 0.9, "y1": 0.25}
    run = redact(tmp_path, fixture("scanned"), boxes=[box], terms=["anything"])
    assert "NO_TEXT_LAYER" in run.warnings
    doc = pdfium.PdfDocument(run.outputs[0])
    image = doc[0].render(scale=1).to_pil().convert("RGB")
    doc.close()
    assert image.getpixel((306, 140)) == (0, 0, 0)


def test_white_fill(tmp_path, fixture) -> None:
    out = redact(tmp_path, fixture("plain"), terms=[SECRET], fill="white").outputs[0]
    assert recoverable(out, [SECRET]) == []


# ------------------------------------------------------- the verifier itself


def run_verify(job_dir: Path, output: str, **request) -> dict:
    payload = {"output": output, "expectedPages": 1, "fill": [0, 0, 0], **request}
    p = subprocess.run(  # noqa: S603 - our interpreter and our verifier
        [sys.executable, "-I", str(VERIFY)],
        input=json.dumps(payload).encode(),
        capture_output=True,
        cwd=job_dir,
        timeout=120,
        check=True,
    )
    return json.loads(p.stdout)


def failed_ids(report: dict) -> set[str]:
    return {c["id"] for c in report["checks"] if not c["passed"]}


@pytest.mark.parametrize("name", ["plain", "rot-90-crop"])
def test_verifier_catches_a_black_rectangle_drawn_over_text(tmp_path, fixture, name) -> None:
    """The key test of the verifier: a box drawn on top, text left underneath."""
    src = fixture(name)
    box = displayed_box(src, SECRET)
    sabotaged = tmp_path / "sabotaged.pdf"
    with pymupdf.open(src) as doc:
        page = doc[0]
        shown = page.rect
        r = pymupdf.Rect(
            box["x0"] * shown.width,
            box["y0"] * shown.height,
            box["x1"] * shown.width,
            box["y1"] * shown.height,
        )
        page.draw_rect(r * page.derotation_matrix, color=(0, 0, 0), fill=(0, 0, 0))
        doc.set_metadata({})
        doc.save(sabotaged, garbage=4)
    report = run_verify(tmp_path, "sabotaged.pdf", terms=[SECRET], regions=[box])
    # V4 also fires when the text is stored as plain bytes; MuPDF writes glyph
    # codes here, so the three text checks are what must catch it.
    assert {"V1", "V2", "V3"} <= failed_ids(report)


def test_both_engines_locate_the_same_region_on_a_rotated_cropped_page(fixture) -> None:
    """V1 maps boxes with MuPDF's matrices; V2 with its own arithmetic from
    pdfium's CropBox and /Rotate. On unredacted text, both must see it."""
    src = fixture("rot-90-crop")
    report = run_verify(src.parent, src.name, regions=[displayed_box(src, SECRET)])
    assert {"V1", "V2"} <= failed_ids(report)
    keep = displayed_box(src, KEEP)
    elsewhere = {**keep, "y0": keep["y0"], "y1": keep["y1"]}
    report = run_verify(src.parent, src.name, regions=[{**elsewhere, "x0": 0.0, "x1": 0.02}])
    assert not {"V1", "V2"} & failed_ids(report)  # an empty strip: nothing under it


def test_verifier_flags_an_incremental_update(tmp_path, fixture) -> None:
    src = fixture("incremental")
    report = run_verify(src.parent, src.name)
    assert "V7" in failed_ids(report)


# ---------------------------------------------------- failure and fallback


def _patched(monkeypatch, verdicts: list[dict | None]):
    """An adapter whose verifier fails as scripted; ``None`` runs the real one."""
    cls = load_adapter_class(ADAPTER)
    module = sys.modules[cls.__module__]
    real = module.run_verifier

    def fake(ctx, output, spec):
        verdict = verdicts.pop(0)
        return real(ctx, output, spec) if verdict is None else verdict

    monkeypatch.setattr(module, "run_verifier", fake)
    return cls()


def _execute(tmp_path: Path, adapter, src: Path, params: dict):
    job_dir, rel = make_job_dir(tmp_path / "job", [src])
    ctx, sent = context(job_dir, rel)
    return job_dir, ctx, sent, lambda: adapter.execute_conversion(ctx, params)


def test_failed_verification_gives_no_output(tmp_path, fixture, monkeypatch) -> None:
    adapter = _patched(monkeypatch, [{"checks": [{"id": "V1", "passed": False, "pages": [1]}]}])
    job_dir, _, _, go = _execute(tmp_path, adapter, fixture("plain"), {"terms": [SECRET]})
    with pytest.raises(AdapterError) as e:
        go()
    assert e.value.code == "E_REDACTION_VERIFICATION_FAILED"
    assert e.value.detail == {"failed": ["V1"], "fallbackAvailable": True}
    assert list((job_dir / "out").iterdir()) == []


def test_rasterize_fallback_burns_in_and_reverifies(tmp_path, fixture, monkeypatch) -> None:
    adapter = _patched(
        monkeypatch, [{"checks": [{"id": "V1", "passed": False, "pages": [1]}]}, None]
    )
    params = {"terms": [SECRET], "rasterizeFallback": True}
    job_dir, _, sent, go = _execute(tmp_path, adapter, fixture("plain"), params)
    [output] = go()
    assert recoverable(output.path, [SECRET]) == []
    assert text_pdfium(output.path).strip() == ""  # the page is an image now
    stages = [m.name for m in sent if getattr(m, "type", None) == "stage"]
    assert "rasterize" in stages and "verified-V8" in stages


def test_a_verifier_that_cannot_report_is_a_failure(tmp_path, fixture, monkeypatch) -> None:
    cls = load_adapter_class(ADAPTER)
    module = sys.modules[cls.__module__]
    broken = tmp_path / "broken.py"
    broken.write_text("print('not json')\n")
    monkeypatch.setattr(module, "VERIFY", broken)
    job_dir, _, _, go = _execute(tmp_path, cls(), fixture("plain"), {"terms": [SECRET]})
    with pytest.raises(AdapterError) as e:
        go()
    assert e.value.detail["failed"] == ["V0"]


# ------------------------------------------------------- page text and params


def test_page_text_is_in_displayed_orientation(tmp_path, fixture) -> None:
    src = fixture("rot-90")
    job_dir, rel = make_job_dir(tmp_path / "job", [src])
    ctx, _ = context(job_dir, rel)
    words = load_adapter_class(ADAPTER)().page_text(ctx, ctx.inputs[0], 1)
    assert (words.width, words.height) == (792, 612)  # landscape as displayed
    keep = next(w for w in words.words if w.text == KEEP)
    assert 0 <= keep.x0 < keep.x1 <= 792 and 0 <= keep.y0 < keep.y1 <= 612
    assert keep.y1 - keep.y0 > keep.x1 - keep.x0  # rotated: the word runs vertically


@pytest.mark.parametrize(
    ("params", "reason"),
    [
        ({}, "nothingToRedact"),
        ({"terms": ["a"]}, "tooShort"),
        ({"boxes": [{"page": 2, "x0": 0, "y0": 0, "x1": 1, "y1": 1}]}, "pageOutOfBounds"),
        ({"boxes": [{"page": 1, "x0": 0.5, "y0": 0, "x1": 0.5, "y1": 1}]}, "empty"),
    ],
)
def test_invalid_params(tmp_path, fixture, params, reason) -> None:
    run = run_in_process(ADAPTER, tmp_path / "job", [fixture("plain")], params)
    assert [e.detail.root["reason"] for e in run.errors] == [reason]
