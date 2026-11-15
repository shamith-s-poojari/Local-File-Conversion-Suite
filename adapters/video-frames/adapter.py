"""video-frames: pictures of a video's frames, a number spread evenly or
one every few seconds (ADR 0004). Each picture is named after its time in
the video (``clip-01m05s.jpg``, or ``clip-00m01.4s.jpg`` when frames
are under a second apart). A camera's rotation flag is applied, so
pictures come out upright. With ``layout: sheet`` the frames of each video
go on one contact sheet, captioned with their times."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import ConversionAdapter, issue
from converter_sdk.engines import binary, run
from converter_sdk.media import CORRUPT_MARKERS, duration_s, probe, video_streams
from converter_sdk.sheets import contact_sheet
from converter_sdk.tables import file_name, unique
from PIL import Image

MB = 1024 * 1024
MAX_FRAMES = 1000
"""Per video. More than this from one video is almost certainly a mistake."""
SHEET_FRAMES = (4, 36)
"""Fewest and most frames on a contact sheet."""


def sheet_columns(n: int) -> int:
    """Columns for a sheet of ``n`` frames: close to 16:9 overall."""
    for limit, cols in ((4, 2), (6, 3), (12, 4), (20, 5)):
        if n <= limit:
            return cols
    return 6


def times(params: dict[str, Any], seconds: float) -> list[float]:
    """When to take each picture."""
    if params.get("mode", "count") == "interval":
        every = int(params.get("every") or 5)
        n = max(1, int(seconds // every) + (1 if seconds % every else 0))
        return [k * every for k in range(n) if k * every < seconds] or [0.0]
    count = int(params.get("count") or 5)
    return [seconds * (k + 0.5) / count for k in range(count)]


def stamp(t: float, tenths: bool = False) -> str:
    """``01m05s``-style label for a time, hours when needed, and tenths of a
    second (``00m01.4s``) when asked."""
    whole = int(t * 10 + 1e-6) if tenths else int(t + 1e-6) * 10
    total, tenth = divmod(whole, 10)
    h, rest = divmod(total, 3600)
    m, s = divmod(rest, 60)
    sec = f"{s:02d}.{tenth}s" if tenths else f"{s:02d}s"
    return f"{h}h{m:02d}m{sec}" if h else f"{m:02d}m{sec}"


def stamps(when: list[float]) -> list[str]:
    """Labels for a video's frame times: whole seconds, or tenths when frames
    are closer than that, so names don't repeat."""
    plain = [stamp(t) for t in when]
    if len(set(plain)) == len(plain):
        return plain
    return [stamp(t, tenths=True) for t in when]


def grab(ctx: JobContext, path: Path, at: float, out: Path, params: dict[str, Any]) -> None:
    """One frame at ``at`` seconds (fast seek, then exact)."""
    width = int(params.get("maxWidth") or 0)
    scale = f"scale='min({width},iw)':-2" if width else "scale='trunc(iw/2)*2':-2"
    argv: list[Any] = [
        binary("ffmpeg"), "-hide_banner", "-nostdin", "-y", "-loglevel", "error",
        "-ss", f"{at:.3f}", "-protocol_whitelist", "file", "-i", os.fspath(path),
        "-map", "0:v:0", "-frames:v", "1", "-vf", scale, "-fflags", "+bitexact",
    ]  # fmt: skip
    if params.get("to", "jpg") == "jpg":
        argv += ["-q:v", "2", "-f", "image2", "-c:v", "mjpeg"]
    else:
        argv += ["-f", "image2", "-c:v", "png"]
    argv.append(os.fspath(out))
    code, err = run(ctx, argv, timeout_s=120)
    if code != 0:
        out.unlink(missing_ok=True)
        if any(m in err.lower() for m in CORRUPT_MARKERS):
            raise AdapterError("E_INPUT_CORRUPT")
        raise AdapterError("E_INTERNAL", {"reason": "engineFailed", "exit": code})
    if not out.is_file() or out.stat().st_size == 0:
        # Seeking to the very end can land after the last frame.
        raise AdapterError("E_INPUT_CORRUPT", {"reason": "noFrame"})


class VideoFramesAdapter(Adapter):
    adapter_id: ClassVar[str] = "video-frames"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "L"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        durations, pixels = [], []
        sheet = params.get("layout", "separate") == "sheet"
        for p in ctx.inputs:
            ctx.check_cancelled()
            info = probe(ctx, p)
            video = video_streams(info)
            if not video:
                return ValidationReport(errors=[issue("inputs", "noVideo")])
            seconds = duration_s(info)
            if len(times(params, seconds)) > MAX_FRAMES:
                return ValidationReport(errors=[issue("every", "tooManyFrames")])
            if sheet and not SHEET_FRAMES[0] <= len(times(params, seconds)) <= SHEET_FRAMES[1]:
                param = "every" if params.get("mode", "count") == "interval" else "count"
                return ValidationReport(errors=[issue(param, "sheetFrames")])
            durations.append(seconds)
            pixels.append(int(video[0].get("width") or 0) * int(video[0].get("height") or 0))
        return ValidationReport(metadata={"durations": durations, "pixels": pixels})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        largest = max(metadata.get("pixels") or [0])
        frames = sum(len(times(params, s)) for s in metadata.get("durations") or [0])
        if params.get("layout", "separate") == "sheet":
            # Frames are decoded at thumbnail size; the sheet is up to 6 × 480 px wide.
            return ResourceEstimate(
                ram_bytes=192 * MB + 36 * 480 * 480 * 3 + 3000 * 2000 * 3 * 3,
                disk_bytes=SHEET_FRAMES[1] * 480 * 480 * 3 + 24 * MB,
                cost_units=max(1, frames),
            )
        return ResourceEstimate(
            ram_bytes=192 * MB + largest * 24,
            disk_bytes=frames * largest * 3 + MB,
            cost_units=max(1, frames),
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        ext = str(params.get("to", "jpg"))
        sheet = params.get("layout", "separate") == "sheet"
        outputs = []
        plan = []
        for i, path in enumerate(ctx.inputs):
            seconds = duration_s(probe(ctx, path))
            plan.append((i, path, times(params, seconds), seconds))
        total = sum(len(t) for _, _, t, _ in plan)
        done = 0
        ctx.stage("frames", 1, 1)
        for i, path, when, seconds in plan:
            base = file_name(ConversionAdapter.title(params, i))
            used: set[str] = set()
            if sheet:
                outputs.append(self.make_sheet(ctx, i, path, base, when, seconds, ext))
                done += len(when)
                ctx.progress("frames", done, total)
                continue
            for t, label in zip(when, stamps(when), strict=True):
                ctx.check_cancelled()
                # Keep clear of the very end, where there may be no frame left.
                at = min(t, max(0.0, seconds - 0.1))
                name = unique(f"{base}-{label}", used, limit=200)
                out = ctx.output_path(f"{name}.{ext}")
                grab(ctx, path, at, out, params)
                outputs.append(OutputFile(out))
                done += 1
                ctx.progress("frames", done, total)
        return outputs

    @staticmethod
    def make_sheet(
        ctx: JobContext,
        index: int,
        path: Path,
        base: str,
        when: list[float],
        seconds: float,
        ext: str,
    ) -> OutputFile:
        """One picture with every frame of ``path`` in a grid."""
        cols = sheet_columns(len(when))
        cell_px = 480 if cols <= 4 else 360
        small = {"to": "png", "maxWidth": cell_px}
        cells: list[tuple[Image.Image, str | None]] = []
        for t, label in zip(when, stamps(when), strict=True):
            ctx.check_cancelled()
            tmp = ctx.temp_path(".png")
            grab(ctx, path, min(t, max(0.0, seconds - 0.1)), tmp, small)
            try:
                with Image.open(tmp) as frame:
                    frame.load()
                    cells.append((frame.copy(), label))
            except (OSError, SyntaxError, ValueError):
                raise AdapterError("E_INPUT_CORRUPT") from None
            finally:
                tmp.unlink(missing_ok=True)
        first = cells[0][0]
        image = contact_sheet(
            cells,
            columns=cols,
            cell_px=cell_px,
            title=base,
            aspect=first.width / first.height,
        )
        out = ctx.output_path(f"{base}-sheet.{ext}")
        if ext == "jpg":
            image.save(out, "JPEG", quality=90, optimize=True)
        else:
            image.save(out, "PNG")
        return OutputFile(out, source=index)


if __name__ == "__main__":
    run_main(VideoFramesAdapter)
