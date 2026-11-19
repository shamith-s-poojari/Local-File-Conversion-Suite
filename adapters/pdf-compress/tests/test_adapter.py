"""Acceptance tests for pdf-compress (ADR 0004)."""

from __future__ import annotations

from pathlib import Path

import pikepdf
from converter_conformance import corpus
from converter_conformance.harness import run_in_process
from pikepdf import Name
from PIL import Image

ADAPTER = Path(__file__).resolve().parents[1]


def photo_pdf(path: Path, side: int = 2400) -> Path:
    """One letter page showing a large, smooth colour photo stored losslessly."""
    img = Image.radial_gradient("L").resize((side, side)).convert("RGB")
    img = Image.merge("RGB", (img.getchannel(0), img.rotate(90).getchannel(0), img.getchannel(0)))
    pdf = pikepdf.new()
    pdf.add_blank_page(page_size=(612, 792))
    page = pdf.pages[0]
    raw = img.tobytes()
    image = pdf.make_stream(
        raw,
        Type=Name.XObject,
        Subtype=Name.Image,
        Width=side,
        Height=side,
        ColorSpace=Name.DeviceRGB,
        BitsPerComponent=8,
    )
    page.Resources = pikepdf.Dictionary(XObject=pikepdf.Dictionary(Im0=image))
    page.contents_add(pdf.make_stream(b"q 500 0 0 500 56 146 cm /Im0 Do Q\n"))
    pdf.save(path, compress_streams=True)
    return path


def _image(path: Path) -> pikepdf.Stream:
    with pikepdf.open(path) as pdf:
        img = next(iter(pdf.pages[0].images.values()))
        return pikepdf.Dictionary({k: img[k] for k in ("/Width", "/Filter")})


def test_balanced_shrinks_pictures_and_the_file(tmp_path: Path) -> None:
    src = photo_pdf(tmp_path / "in.pdf")
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"fileTitles": ["scan"]})
    assert not r.errors
    assert r.names == ["scan-compressed.pdf"]
    assert r.outputs[0].stat().st_size < src.stat().st_size / 4
    info = _image(r.outputs[0])
    assert int(info.Width) == 150 * 11  # 11 in is the page's long side
    assert info.Filter == Name.DCTDecode
    assert "IMAGE_DOWNSCALED" in r.warnings


def test_strong_is_smaller_than_balanced(tmp_path: Path) -> None:
    src = photo_pdf(tmp_path / "in.pdf")
    balanced = run_in_process(ADAPTER, tmp_path / "a", [src], {"level": "balanced"})
    strong = run_in_process(ADAPTER, tmp_path / "b", [src], {"level": "strong"})
    assert strong.outputs[0].stat().st_size < balanced.outputs[0].stat().st_size


def test_lossless_leaves_pictures_alone(tmp_path: Path) -> None:
    src = photo_pdf(tmp_path / "in.pdf", side=400)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"level": "lossless"})
    info = _image(r.outputs[0])
    assert int(info.Width) == 400
    assert info.Filter != Name.DCTDecode


def test_a_file_that_cant_shrink_is_kept(tmp_path: Path) -> None:
    src = corpus.build("text-1", tmp_path / "corpus")
    # Already packed: rewriting it again can't save anything.
    packed = tmp_path / "packed.pdf"
    with pikepdf.open(src) as pdf:
        pdf.save(
            packed, object_stream_mode=pikepdf.ObjectStreamMode.generate, recompress_flate=True
        )
    r = run_in_process(ADAPTER, tmp_path / "job", [packed], {})
    if "KEPT_ORIGINAL" in r.warnings:
        assert r.outputs[0].read_bytes() == packed.read_bytes()
    assert r.outputs[0].stat().st_size <= packed.stat().st_size


def test_cmyk_pictures_are_left_alone(tmp_path: Path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    image = pdf.make_stream(
        bytes(800 * 800 * 4),
        Type=Name.XObject,
        Subtype=Name.Image,
        Width=800,
        Height=800,
        ColorSpace=Name.DeviceCMYK,
        BitsPerComponent=8,
    )
    pdf.pages[0].Resources = pikepdf.Dictionary(XObject=pikepdf.Dictionary(Im0=image))
    pdf.pages[0].contents_add(pdf.make_stream(b"q 400 0 0 400 100 100 cm /Im0 Do Q\n"))
    src = tmp_path / "cmyk.pdf"
    pdf.save(src)
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {"level": "strong"})
    assert int(_image(r.outputs[0]).Width) == 800


def test_a_picture_declaring_a_huge_size_is_not_decoded(monkeypatch) -> None:
    """P3: the declared size is checked before pikepdf decodes anything."""
    import zlib  # noqa: PLC0415

    from converter_conformance.harness import load_adapter_class  # noqa: PLC0415

    module = __import__("sys").modules[load_adapter_class(ADAPTER).__module__]
    pdf = pikepdf.new()
    stream = pdf.make_stream(
        zlib.compress(b"\0" * 64),
        Type=Name.XObject,
        Subtype=Name.Image,
        Width=100_000,
        Height=100_000,
        ColorSpace=Name.DeviceGray,
        BitsPerComponent=8,
        Filter=Name.FlateDecode,
    )
    assert module.candidate(stream)
    decoded = []

    class Spy:
        def __init__(self, obj) -> None:
            decoded.append(obj)
            raise RuntimeError("must not be reached")

    monkeypatch.setattr(pikepdf, "PdfImage", Spy)
    assert module.recompress(stream, 800, 75) == (False, False)
    assert decoded == []


def test_encrypted_input_gives_encryption_removed(tmp_path: Path) -> None:
    """P6: the compressed copy is not protected, and the user is told."""
    src = corpus.encrypted_pdf(tmp_path / "enc.pdf")
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {}, {"password": corpus.SENTINEL_PASSWORD})
    assert not r.errors
    assert r.warnings.count("ENCRYPTION_REMOVED") == 1
