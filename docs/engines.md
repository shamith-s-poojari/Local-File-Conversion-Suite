# Engines: what the app looks for, and where

Most conversions run on Python libraries that are part of the app's own runtime. Two programs are not: **FFmpeg** (audio and video) and **LibreOffice** (Word, Excel, PowerPoint and OpenDocument files). Neither is bundled. The app looks for them when it starts, in the places below, and offers only the conversions it can run. [Troubleshooting](troubleshooting.md) has the fixes when one isn't found.

This page describes the code in `crates/app-core/src/engines.rs` (the search) and `workers/sdk/src/converter_sdk/engines.py` (how a worker gets the result).

## How an engine reaches a worker

1. At start-up the core looks for each engine (`Engines::discover`). It never downloads one.
2. An engine that is found gets a **grant** for the sandbox: read access to its install folder, permission to run the named program, and nothing more.
3. The core puts the program's full path in the worker's environment (`LFCS_BIN_FFMPEG`, `LFCS_BIN_FFPROBE`, `LFCS_BIN_SOFFICE`; `LFCS_BIN_GS` and `LFCS_BIN_TESSERACT` for Ghostscript and Tesseract, which no shipped adapter lists today). Adapters never search `PATH` themselves, and only an adapter that lists the engine in `requires` in its `manifest.yaml` gets its grant.
4. A worker starts an engine with a minimal environment: its job folder as `HOME` and `TMPDIR`, system folders on `PATH`, and a few Windows variables. Nothing else from the worker's environment is passed on.

If an engine isn't found, the conversions that need it are shown as unavailable, with the install command for the system.

## FFmpeg

Both `ffmpeg` and `ffprobe` must be found. The search goes in this order and takes the first match:

1. Every **absolute** entry of `PATH`. An empty or relative entry is ignored, because a shell reads it as "the current folder" and a hostile folder could supply the program.
2. The fixed folders for the system:

| System | Folders |
|---|---|
| macOS | `/opt/homebrew/opt/ffmpeg-full/bin`, `/usr/local/opt/ffmpeg-full/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `/bin` |
| Linux | the same list (the two `ffmpeg-full` folders only exist with Homebrew on Linux) |
| Windows | WinGet's links (`%LOCALAPPDATA%\Microsoft\WinGet\Links`); Scoop (`%SCOOP%` or `%USERPROFILE%\scoop`, `apps\ffmpeg\current` and its `bin`); Chocolatey's package folders (`%ChocolateyInstall%\lib\ffmpeg` and `ffmpeg-full`, `tools\<unpacked folder>\bin`); `%ProgramFiles%\ffmpeg\bin`; `C:\ffmpeg\bin` |

What is accepted:

- The path is resolved to the real file. A name that resolves to a different program (Snap's `snap` launcher, say) is skipped.
- Chocolatey's shims (`<root>\bin\ffmpeg.exe`) are skipped: they start the real program as a second process from a folder the sandbox doesn't grant. The package's own folder is found instead. Scoop's shims are followed to the real binary.
- The install folder is shared read-only so the program can load its libraries. Known layouts: Homebrew's `Cellar` (all kegs), `/usr`, `/usr/local`, `/opt/<name>`, `C:\Program Files\<name>`, and an `.app` bundle. Any other location shares its own folder and a sibling `lib`. A program whose folder would be your home folder or `/` (a `ffmpeg` dropped into `~/bin` directly under home, say) is treated as **not installed**.

**Ogg.** Writing Ogg (Vorbis) needs an FFmpeg built with libvorbis. Homebrew's `ffmpeg` formula leaves it out. `ffmpeg-full` has it but isn't linked into `bin`, so its own folder is in the list above. `PATH` is searched first, so a plain `ffmpeg` that is on `PATH` wins over `ffmpeg-full`. A macOS app started from the Finder has a short `PATH` (`/usr/bin:/bin` and a few more), so there `ffmpeg-full` is the one found; from a terminal, check `which ffmpeg`.

## LibreOffice

LibreOffice is **not** searched for on `PATH`. The app runs LibreOffice's own binary directly (the `soffice` launcher script needs a shell and starts a second program, which the sandbox doesn't allow), so it looks for exactly these files and takes the first that exists:

| System | Files |
|---|---|
| macOS | `/Applications/LibreOffice.app/Contents/MacOS/soffice`, then `~/Applications/LibreOffice.app/Contents/MacOS/soffice` |
| Windows | `%ProgramFiles%\LibreOffice\program\soffice.com` and `soffice.exe`, then the same under `%ProgramFiles(x86)%` |
| Linux | `/usr/lib/libreoffice/program/soffice.bin`, `/usr/lib64/libreoffice/program/soffice.bin`, then `/opt/libreoffice*/program/soffice.bin`, newest name first |

Notes:

- A Snap or Flatpak LibreOffice isn't in these places and isn't found. Use the distribution's package (`libreoffice-core` with `libreoffice-writer`, `-calc` and `-impress`) or the official download.
- A LibreOffice in some other folder isn't found either. There is no setting for a custom path.
- LibreOffice exits with code 81 the first time it creates a profile and asks to be started again. The launcher script normally does that; because the app runs the binary directly, the worker starts it a second time itself (`converter_sdk/office.py`). This is handled; you don't need to do anything.
- Each job gets its own LibreOffice profile in its work folder, so jobs never share state and nothing is written to your own LibreOffice profile.
- Fonts the system lacks are substituted. Results depend on the installed version and fonts.

## Other engines

- **Python libraries** (pikepdf, PyMuPDF, pypdfium2, Pillow, pillow-heif, ReportLab, openpyxl, lxml, markdown-it-py, PyYAML, pdf2docx) are found by importing them in the worker's Python. They come with the runtime: the checkout's `.venv` in development, the bundled `python/` folder in a release build (see [Release plan](release-plan.md)). The catalog of approved engines is `engines/engines.yaml`.
- **Ghostscript and Tesseract** are looked for (`gs` or `gswin64c`, `tesseract`, by the same rules as FFmpeg) but no shipped adapter lists them.

## Versions the tests ran against

The engines aren't pinned: CI installs whatever the runner's package manager provides, and each job prints the versions it got (`tools/engine-versions.py`, also written to the job summary). Look there for what a given run used.

| System | FFmpeg (CI install) | LibreOffice (CI install) |
|---|---|---|
| Linux (Ubuntu 24.04) | the `ffmpeg` package | the `libreoffice-core`, `-writer`, `-calc` and `-impress` packages: 24.2.7 on the 2026-11-18 run |
| macOS | Homebrew `ffmpeg-full` | Homebrew cask `libreoffice` |
| Windows | Chocolatey `ffmpeg` | Chocolatey `libreoffice-fresh` |

Only the Linux LibreOffice version is recorded above, because it is the one that was in a log when this page was written. For the others, read the "Engine versions" step of the latest CI run. Results with other versions of either program haven't been tested. Pinning versions with checksums is a release blocker ([release plan](release-plan.md#6-pinned-engine-versions-with-checksums)).
