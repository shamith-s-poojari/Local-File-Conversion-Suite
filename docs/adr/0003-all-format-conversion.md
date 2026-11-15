# ADR 0003: Convert between every common document, image, audio and video format

- Status: accepted
- Date: 2026-11-13
- Supersedes, in part: the blueprint's format list (12 formats) and its "no audio/video" line; ADR 0001's "the sniffer stays at 12 formats in v1"

## Context

The product owner asked for an all-round converter instead of a PDF toolkit: Word to PDF, PDF to JPEG or GIF, MP3 to WAV or FLAC and back, and so on, "every file to another related file". The blueprint deliberately left audio and video out ("FFmpeg … no audio/video in scope") and kept the sniffer at 12 formats.

Everything else in the blueprint still holds: no network, every parser in the per-job sandbox, outputs re-sniffed before they can be saved, AGPL.

## Decision

### Formats (41)

| Family | Formats |
|---|---|
| Documents | PDF, DOCX, DOC, ODT, RTF, PPTX, PPT, ODP, XLSX, XLS, ODS, CSV, Markdown, HTML, TXT, EPUB, JSON, YAML |
| Images | PNG, JPG, WebP, SVG, HEIC, GIF, BMP, TIFF, AVIF, ICO |
| Audio | MP3, WAV, FLAC, OGG (Vorbis), Opus, M4A (AAC/ALAC), AAC (ADTS), AIFF |
| Video | MP4, MOV, MKV, WebM, AVI |

Each is identified from content (magic numbers, ZIP `mimetype`, CFB stream names, `ftyp` brands, EBML DocType, MPEG frame headers), with the extension as a tie-breaker only where formats overlap (MP4/M4A brands, CSV, YAML).

### Routing

The blueprint matrix still governs pairs among its 12 formats. A pair involving a new format is offered when an adapter declares a **direct** edge for it, at the declared tier. Every offered conversion runs as one job, in one adapter per kind of file; an adapter that needs an intermediate (DOCX → PNG through PDF) does it internally.

A mixed selection is offered every target all its kinds share, even when they convert with different adapters: Word files and photos "all to PDF", songs and videos "all to MP3". The job then runs in parts, one per adapter, one after another under a single memory admission (the largest part's estimate) and in the strictest of their pools. Every part is validated before any runs. Each worker numbers its outputs from 0, so the core renames a part's outputs (`p0-0.pdf`) before the next part starts, and checks every output's hash, path and format only after the last worker has exited, so no worker can change another part's file unnoticed. Each part gets only the parameters its own schema knows; the UI sends none and says each kind uses its usual settings.

### Engines

| Engine | Use | License category | New? |
|---|---|---|---|
| FFmpeg (separate process) | Audio and video | gpl-separate-process | **Yes** |
| LibreOffice (separate process) | Office and OpenDocument, RTF, HTML → PDF/Office | weak-copyleft | Already approved |
| pdf2docx | PDF → Word | agpl | Already approved |
| openpyxl | Excel ↔ CSV/JSON/YAML | permissive | Already approved |
| PyYAML (safe loader only) | YAML | permissive | **Yes** |
| pillow-heif | HEIC input | weak-copyleft | Already approved |
| markdown-it-py | Markdown → HTML/PDF/Word | permissive | **Yes** |

External binaries are discovered at startup. A missing engine makes its conversions show as unavailable with the reason, and the home screen shows that OS's usual install command; nothing is downloaded.

Where each OS looks, after `PATH`:

| Engine | macOS | Windows | Linux |
|---|---|---|---|
| FFmpeg, FFprobe | Homebrew (`/opt/homebrew/bin`, `/usr/local/bin`) | WinGet links, Scoop and Chocolatey package folders (never their shims, which start the real binary as a second process), `Program Files\ffmpeg\bin`, `C:\ffmpeg\bin` | `/usr/bin`, `/usr/local/bin` |
| LibreOffice | `LibreOffice.app` in `/Applications` or `~/Applications` | `Program Files\LibreOffice\program\soffice.com` (it waits for the conversion), then `soffice.exe` | `program/soffice.bin` under `/usr/lib`, `/usr/lib64` or `/opt/libreoffice*`, run directly: the `soffice` script needs a shell and `oosplash`, which the sandbox doesn't have |

A binary that resolves to a different program (Snap's `snap` launcher) is skipped, since it can't run in the sandbox.

### Sandbox

Workers that need an external binary get a per-adapter grant: read the engine's install, execute only that engine's binaries, the engine's path in the environment, and how many processes it runs. Adapters that don't list the engine in `requires` don't get the grant. Network stays denied for every process in the tree.

- **macOS (Seatbelt).** The grant adds file-read and map, process-exec for the engine's binaries, and for LibreOffice 20 vetted Mach services (fonts, preferences, the window server it registers with even headless) plus its single-instance pipe in `/tmp`: create and accept only, never connect out. Services that reach user data or other apps (clipboard, Apple Events, recent documents, LaunchServices writes, privacy prompts) stay denied.
- **Linux (bubblewrap).** The install is bound read-only; each job has a private `/tmp`.
- **Windows (AppContainer and Job Object, T09).** The runner starts the worker in a per-job AppContainer with no capabilities and only the job directory granted, inside a Job Object with the memory limit, a process count and UI restrictions. Engine installs and the runtime must be readable by `ALL APPLICATION PACKAGES`, as Program Files is; the app adds that once to folders that lack it (a per-user install, a WinGet package). **Compile-checked only: it hasn't run on Windows yet.**

Process counts (spec 9.2, "1 for PDF workers; set per engine"): a worker may run itself, `limits.processes - 1` helpers from its manifest (`pdf-redact` runs a verifier), and its engine's processes (FFmpeg 1, LibreOffice 3). On Windows a virtualenv's `python.exe` is a launcher, so development adds one. Windows enforces the count; macOS limits what may be executed and Linux gives each worker a PID namespace.

Memory is measured as macOS's physical footprint (what Activity Monitor shows), not resident size, which counts shared system frameworks a GUI engine maps; available memory there is free plus inactive plus speculative pages. Linux uses RSS and Windows the Job Object's committed-memory limit.

### Data files (CSV, JSON, YAML, Excel)

`data-convert` moves data between CSV, JSON, YAML and Excel without LibreOffice, and lays CSV out as a Markdown or HTML table. Between Excel and CSV it outranks LibreOffice (tier R against L), and LibreOffice stays as the fallback for CSV → Excel when openpyxl is missing. Excel → Markdown stays off, as the matrix says.

- **Tables and trees.** A table becomes a list of records keyed by the header row (or a list of rows when "First row holds column names" is off). A workbook with several sheets becomes an object keyed by sheet name. Going the other way, a list of records becomes one table and an object of lists one table per key; a value that is itself a list or object is written as JSON text in its cell (`NESTED_DATA_FLATTENED`).
- **Nothing changes meaning.** A cell becomes a number only when writing it back gives the same text: `007`, `1e5`, `7,5` and numbers of more than 15 digits stay text. Text that starts with `=`, `+`, `-` or `@` is written to Excel as a text cell, never a formula. Excel formulas are read as their last saved values (`FORMULAS_DROPPED`). YAML dates, binary and non-string keys become plain strings, and comments are lost (`COMMENTS_DROPPED`).
- **Limits.** Only YAML's safe tags load; any other is `E_INPUT_CORRUPT`. Alias expansion and cell counts are capped (`E_LIMIT_EXCEEDED`); nesting deeper than 500 levels or a self-referencing YAML document is `E_INPUT_CORRUPT`. Workbooks are checked for their unpacked size before openpyxl opens them.
- **CSV.** The separator (comma, semicolon, tab or pipe) and the encoding (UTF-8 with or without a BOM, UTF-16 with a BOM, else Windows-1252) are detected; CSV written by the app uses the chosen separator, UTF-8 and `\n`.

### Text that isn't there

A PDF without a text layer (a scan) can't become text. Converting one to TXT, Markdown, HTML or Excel fails with `E_NO_TEXT`, which says so and suggests Word (which keeps the page images) or images instead. A PDF that MuPDF had to repair to open gives `E_INPUT_CORRUPT` instead, since the text was more likely lost to damage.

### Output is the same on every OS

Text outputs are written with `\n` line endings everywhere, workers read and write UTF-8 regardless of the system code page (`PYTHONUTF8=1`), and PDF outputs are rewritten without their `/ID` and LibreOffice's `/DocChecksum`, so a conversion repeated on the same input gives the same bytes. Workers use Python's built-in MIME table, never `/etc/mime.types` or the Windows registry, which differ between machines (and which the sandbox doesn't let them read).

## Consequences

- The conformance kit grows media and office fixtures; outputs of every new format are re-sniffed, so the sniffer must recognise everything an adapter can write.
- Packaging (T21) must bundle FFmpeg and LibreOffice per platform, or document them as prerequisites. Until then they're used from the system when present.
- Video conversion is CPU-heavy. Jobs use the `media` worker pool, one at a time by default.
- Windows and Linux builds compile and pass the lint and type checks, but the Windows sandbox and the Linux LibreOffice path have only run in CI-style cross checks, not on those OSes. Before relying on them, the sandbox probe (`self_test`) and `cargo test -p converter-app-core --test end_to_end` must pass on Windows 10/11 and Ubuntu 24.04. A failing probe keeps processing disabled, so the risk is an app that refuses to convert, never one that runs unsandboxed.
- LibreOffice in an AppContainer is the least certain part (spec C1 allows a low-integrity fallback for it only, with its own ADR). If it can't start there, its conversions fail with a sandbox error and everything else still works.
