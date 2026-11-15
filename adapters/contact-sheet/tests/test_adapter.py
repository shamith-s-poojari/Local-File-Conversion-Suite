"""Acceptance tests for contact-sheet (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import yaml
from converter_conformance import corpus, files
from converter_conformance.harness import run_in_process
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]


def run(tmp_path: Path, srcs: list[Path], secrets=None, **params):
    return run_in_process(ADAPTER, tmp_path / "job", srcs, {"to": "png", **params}, secrets=secrets)


def make_images(tmp_path: Path, formats=("png", "jpg", "gif")) -> list[Path]:
    return [files.image_file(tmp_path / f"in{i}.{f}", f) for i, f in enumerate(formats)]


def ink_below(img: Image.Image, box: tuple[int, int, int, int]) -> bool:
    return img.crop(box).convert("L").getextrema()[0] < 120


def test_params_schema_matches_manifest() -> None:
    manifest = yaml.safe_load((ADAPTER / "manifest.yaml").read_text("utf-8"))
    assert manifest["outputs"] == {"format": "chosen", "cardinality": "many"}
    assert "ffmpeg" not in manifest["requires"]


def test_mixed_inputs_make_one_a4_picture(tmp_path: Path) -> None:
    srcs = make_images(tmp_path, ("png", "jpg", "gif", "bmp", "tiff"))
    srcs.append(corpus.text_pdf(tmp_path / "doc.pdf", 3))
    r = run(tmp_path, srcs, fileTitles=["a", "b", "c", "d", "e", "doc"])
    assert not r.errors
    assert r.names == ["contact-sheet.png"]
    with Image.open(r.outputs[0]) as img:
        assert img.format == "PNG"
        assert img.size == (1240, 1754)  # A4 at 150 dpi


def test_jpg_and_sizes(tmp_path: Path) -> None:
    srcs = make_images(tmp_path, ("png", "jpg"))
    for sheet, size in (("a4", (1240, 1754)), ("letter", (1275, 1650))):
        r = run(tmp_path / sheet, srcs, to="jpg", sheet=sheet)
        assert r.names == ["contact-sheet.jpg"]
        with Image.open(r.outputs[0]) as img:
            assert img.format == "JPEG" and img.size == size
    r = run(tmp_path / "screen", srcs, to="jpg", sheet="screen")
    with Image.open(r.outputs[0]) as img:
        assert img.width == 1920 and img.height < 1920


def test_many_cells_make_several_sheets(tmp_path: Path) -> None:
    src = corpus.text_pdf(tmp_path / "doc.pdf", 40)
    r = run(tmp_path, [src], columns=8, sheet="screen", fileTitles=["doc"])
    assert not r.errors
    assert r.names == ["contact-sheet.png"]  # 40 pages, 8 across, 10 rows: one sheet
    r = run(tmp_path / "b", [src], columns=3, sheet="letter", fileTitles=["doc"])
    assert len(r.outputs) > 1
    assert r.names == [f"contact-sheet-{i + 1}.png" for i in range(len(r.outputs))]


def test_pdf_output_has_a_page_per_sheet(tmp_path: Path) -> None:
    import pikepdf  # noqa: PLC0415

    src = corpus.text_pdf(tmp_path / "doc.pdf", 40)
    r = run(tmp_path, [src], to="pdf", columns=3, sheet="letter")
    assert not r.errors
    assert r.names == ["contact-sheet.pdf"]
    with pikepdf.open(r.outputs[0]) as pdf:
        assert len(pdf.pages) > 1
        box = [float(v) for v in pdf.pages[0].MediaBox]
        assert box[2:] == [612.0, 792.0]
    (tmp_path / "imgs").mkdir()
    one = run(tmp_path / "b", make_images(tmp_path / "imgs"), to="pdf", sheet="a4")
    with pikepdf.open(one.outputs[0]) as pdf:
        assert len(pdf.pages) == 1
        assert round(float(pdf.pages[0].MediaBox[2]), 1) == 595.3


def test_captions_on_and_off(tmp_path: Path) -> None:
    src = [files.image_file(tmp_path / "in.png", "png")]
    on = run(tmp_path / "a", src, columns=3, fileTitles=["holiday"])
    off = run(tmp_path / "b", src, columns=3, captions=False, fileTitles=["holiday"])

    def inked_rows(path: Path) -> set[int]:
        with Image.open(path) as img:
            gray = img.convert("L")
            return {
                y
                for y in range(gray.height)
                if gray.crop((0, y, gray.width, y + 1)).getextrema()[0] < 100
            }

    # The test picture has a dark label inside the thumbnail; the caption adds
    # dark rows below it, so the captioned sheet has rows the other lacks.
    assert inked_rows(on.outputs[0]) - inked_rows(off.outputs[0])


def test_order_is_the_input_order(tmp_path: Path) -> None:
    red = tmp_path / "red.png"
    blue = tmp_path / "blue.png"
    Image.new("RGB", (100, 100), (255, 0, 0)).save(red)
    Image.new("RGB", (100, 100), (0, 0, 255)).save(blue)

    def colours_left_to_right(srcs: list[Path], where: Path) -> list[tuple[int, int, int]]:
        r = run(where, srcs, captions=False, columns=3, sheet="screen")
        with Image.open(r.outputs[0]) as img:
            row = img.convert("RGB").crop((0, 300, img.width, 301))
            seen = []
            for px in row.getdata():
                if px in ((255, 0, 0), (0, 0, 255)) and (not seen or seen[-1] != px):
                    seen.append(px)
            return seen

    assert colours_left_to_right([red, blue], tmp_path / "a") == [(255, 0, 0), (0, 0, 255)]
    assert colours_left_to_right([blue, red], tmp_path / "b") == [(0, 0, 255), (255, 0, 0)]


def test_password_protected_pdf(tmp_path: Path) -> None:
    src = corpus.encrypted_pdf(tmp_path / "locked.pdf")
    locked = run(tmp_path / "a", [src])
    assert locked.errors and locked.errors[0].code == "E_PASSWORD_REQUIRED"
    wrong = run(tmp_path / "w", [src], secrets={"password": "nope"})
    assert wrong.errors and wrong.errors[0].code == "E_PASSWORD_WRONG"
    opened = run(tmp_path / "b", [src], secrets={"password": corpus.SENTINEL_PASSWORD})
    assert not opened.errors and len(opened.outputs) == 1


def test_limits_are_validation_issues(tmp_path: Path) -> None:
    long = corpus.text_pdf(tmp_path / "long.pdf", 501)
    r = run(tmp_path / "a", [long])
    assert r.errors and r.errors[0].code == "E_LIMIT_EXCEEDED"
    assert r.errors[0].detail.root["limit"] == "pages"
    full = corpus.text_pdf(tmp_path / "full.pdf", 500)
    r = run(tmp_path / "b", [full] * 4 + [make_images(tmp_path, ("png",))[0]])
    assert r.errors and r.errors[0].detail.root["limit"] == "thumbnails"


def test_bad_params_are_param_errors(tmp_path: Path) -> None:
    src = make_images(tmp_path, ("png",))
    assert run(tmp_path / "a", src, columns=2).errors[0].param == "columns"
    assert run(tmp_path / "b", src, sheet="a0").errors[0].param == "sheet"
