#!/usr/bin/env python3
"""Print the FFmpeg and LibreOffice versions this machine has (REL-5).

CI runs it in each job so the log, and the job summary, say which engine
versions the tests ran against. Engines come from the runner's package
manager and are not pinned.

    python3 tools/engine-versions.py [--require ffmpeg] [--require soffice]

LibreOffice is looked for in the folders the app itself searches
(`crates/app-core/src/engines.rs`, `soffice_candidates`), so a pass here means
the app can find it too. FFmpeg is looked for on PATH and in the Homebrew
`ffmpeg-full` folders. With `--require NAME` a missing engine, or one that
won't print a version, exits with 1; without it nothing here fails the job.
Standard library only.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


def ffmpeg_candidates() -> list[str]:
    found = [
        "/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg",
        "/usr/local/opt/ffmpeg-full/bin/ffmpeg",
    ]
    on_path = shutil.which("ffmpeg")
    return ([on_path] if on_path else []) + [p for p in found if Path(p).is_file()]


def soffice_candidates() -> list[str]:
    """The same places as `soffice_candidates` in engines.rs, plus the
    `soffice` launcher on Linux (it prints the version; soffice.bin needs
    the launcher's environment)."""
    out: list[str] = []
    if sys.platform == "darwin":
        out.append("/Applications/LibreOffice.app/Contents/MacOS/soffice")
        home = os.environ.get("HOME")
        if home:
            out.append(f"{home}/Applications/LibreOffice.app/Contents/MacOS/soffice")
    elif sys.platform == "win32":
        for var in ("ProgramFiles", "ProgramFiles(x86)"):
            root = os.environ.get(var)
            if root:
                out.append(str(Path(root, "LibreOffice", "program", "soffice.com")))
                out.append(str(Path(root, "LibreOffice", "program", "soffice.exe")))
    else:
        out += [
            "/usr/lib/libreoffice/program/soffice.bin",
            "/usr/lib64/libreoffice/program/soffice.bin",
        ]
        opt = Path("/opt")
        if opt.is_dir():
            out += [
                str(p / "program" / "soffice.bin")
                for p in sorted(opt.glob("libreoffice*"), reverse=True)
            ]
    return [p for p in out if Path(p).is_file()]


def version_line(exe: str, flag: str, timeout: float = 90.0) -> str | None:
    try:
        r = subprocess.run(  # noqa: S603 - fixed arguments, a path found above
            [exe, flag],
            capture_output=True,
            text=True,
            timeout=timeout,
            stdin=subprocess.DEVNULL,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    lines = (r.stdout or r.stderr).strip().splitlines()
    return lines[0].strip() if lines and r.returncode == 0 else None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--require", action="append", default=[], choices=["ffmpeg", "soffice"])
    args = ap.parse_args()

    report: dict[str, str] = {}
    for name, found, flag in (
        ("ffmpeg", ffmpeg_candidates(), "-version"),
        ("soffice", soffice_candidates(), "--version"),
    ):
        if not found:
            report[name] = "not found"
            continue
        line = version_line(found[0], flag)
        if line is None and name == "soffice" and sys.platform not in ("darwin", "win32"):
            launcher = shutil.which("soffice")
            line = version_line(launcher, flag) if launcher else None
        report[name] = f"{line}  [{found[0]}]" if line else f"found at {found[0]} but no version"

    for name, text in report.items():
        print(f"{name}: {text}")

    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        job = os.environ.get("GITHUB_JOB", "job")
        os_name = os.environ.get("RUNNER_OS", sys.platform)
        with open(summary, "a", encoding="utf-8") as f:
            f.write(f"Engine versions, {job} on {os_name}\n\n")
            for name, text in report.items():
                f.write(f"- {name}: {text.split('  [')[0]}\n")
            f.write("\n")

    missing = [n for n in args.require if report[n] == "not found" or "no version" in report[n]]
    if missing:
        print(f"required engine missing or unusable: {', '.join(missing)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
