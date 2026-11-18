"""media-clean cleans the picture inside cover art, not only the stream's tags
(audit PRIV-3), and checks for cancel while it works (PY-3)."""

from __future__ import annotations

import io
import os
import subprocess
import sys
from pathlib import Path

import pytest
from converter_conformance import files
from converter_conformance.harness import context, load_adapter_class, run_in_process
from converter_sdk import reveal
from converter_sdk.media import probe
from PIL import Image
from PIL.PngImagePlugin import PngInfo

ADAPTER = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(
    files.ffmpeg() is None and not os.environ.get("CI"), reason="needs ffmpeg"
)


def jpeg_cover(path: Path) -> Path:
    exif = Image.Exif()
    exif[0x010F], exif[0x0110], exif[0x013B] = "Acme", "Acme X100", "Jane Roe"
    exif.get_ifd(0x8825).update({1: "N", 2: (37.0, 46.0, 29.64), 3: "W", 4: (122.0, 25.0, 9.84)})
    Image.new("RGB", (32, 32), (0, 128, 0)).save(path, "JPEG", exif=exif.tobytes())
    return path


def png_cover(path: Path) -> Path:
    info = PngInfo()
    info.add_text("Author", "Jane Roe")
    Image.new("RGB", (32, 32), (0, 0, 128)).save(path, "PNG", pnginfo=info)
    return path


def with_cover(tmp_path: Path, fmt: str, cover: Path) -> Path:
    codec = {"mp3": "libmp3lame", "m4a": "aac", "flac": "flac"}[fmt]
    src = tmp_path / f"in.{fmt}"
    subprocess.run(  # noqa: S603
        [str(files.ffmpeg()), "-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=1",
         "-i", str(cover), "-map", "0", "-map", "1", "-c:a", codec, "-c:v", "copy",
         "-disposition:v", "attached_pic", str(src)],
        check=True, capture_output=True,
    )  # fmt: skip
    return src


def cover_bytes(tmp_path: Path, path: Path) -> bytes:
    (tmp_path / "pj").mkdir(exist_ok=True)
    ctx, _ = context(tmp_path / "pj", [])
    info = probe(ctx, path)
    idx = next(s["index"] for s in info["streams"] if s["disposition"]["attached_pic"])
    out = tmp_path / f"cover-{path.stem}.img"
    subprocess.run(  # noqa: S603
        [str(files.ffmpeg()), "-v", "error", "-y", "-i", str(path), "-map", f"0:{idx}",
         "-c", "copy", "-f", "image2", "-update", "1", str(out)],
        check=True, capture_output=True,
    )  # fmt: skip
    return out.read_bytes()


@pytest.mark.parametrize(
    ("fmt", "make"),
    [
        ("mp3", jpeg_cover),
        ("m4a", jpeg_cover),
        ("flac", jpeg_cover),
        ("mp3", png_cover),
    ],  # fmt: skip
)
def test_cover_picture_loses_its_metadata_and_keeps_its_pixels(
    tmp_path: Path, fmt: str, make
) -> None:
    cover = make(tmp_path / "cover.img")
    src = with_cover(tmp_path, fmt, cover)
    before = cover_bytes(tmp_path, src)
    assert reveal.image_bytes_findings(before), "the fixture must carry metadata"
    r = run_in_process(ADAPTER, tmp_path / "job", [src], {})
    assert not r.errors
    after = cover_bytes(tmp_path, r.outputs[0])
    assert reveal.image_bytes_findings(after) == []
    assert b"Jane" not in after and b"Exif" not in after
    with Image.open(io.BytesIO(before)) as old, Image.open(io.BytesIO(after)) as new:
        assert list(old.convert("RGB").getdata()) == list(new.convert("RGB").getdata())
    (tmp_path / "pj").mkdir(exist_ok=True)
    info = probe(context(tmp_path / "pj", [])[0], r.outputs[0])
    assert [s["codec_type"] for s in info["streams"]] == ["audio", "video"]
    assert info["streams"][1]["disposition"]["attached_pic"] == 1


def test_clean_args_reattach_cleaned_covers_and_drop_the_rest() -> None:
    module = sys.modules[load_adapter_class(ADAPTER).__module__]
    info = {"streams": [
        {"index": 0, "codec_type": "audio", "tags": {}},
        {"index": 1, "codec_type": "video", "codec_name": "mjpeg", "tags": {},
         "disposition": {"attached_pic": 1}},
        {"index": 2, "codec_type": "video", "codec_name": "bmp", "tags": {},
         "disposition": {"attached_pic": 1}},
    ]}  # fmt: skip
    cleanable, unsupported = module.covers_to_clean(info)
    assert [s["index"] for s in cleanable] == [1] and unsupported == [2]
    args = module.clean_args(info, False, replaced=[1], dropped=unsupported)
    assert args[:6] == ["-map", "0", "-map", "-0:1", "-map", "-0:2"]
    assert ["-map", "1:0", "-disposition:v:0", "attached_pic"] == args[6:10]
    assert module.clean_args(info, True)[:6] == ["-map", "0", "-map", "-0:1", "-map", "-0:2"]


def test_cover_loops_check_for_cancel(tmp_path: Path, monkeypatch) -> None:
    from converter_sdk.errors import Cancelled

    module = sys.modules[load_adapter_class(ADAPTER).__module__]
    src = with_cover(tmp_path, "mp3", jpeg_cover(tmp_path / "cover.jpg"))
    (tmp_path / "job").mkdir()
    ctx, _ = context(tmp_path / "job", [])
    info = probe(ctx, src)
    ctx._cancel.set()
    with pytest.raises(Cancelled):
        module.extract_clean_cover(ctx, src, module.covers_to_clean(info)[0][0])
