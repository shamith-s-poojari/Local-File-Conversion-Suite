# ADR 0005: Safety, privacy and convenience

- Status: accepted
- Date: 2026-11-14
- Builds on: [ADR 0003](0003-all-format-conversion.md) (formats and engines), [ADR 0004](0004-tools-for-every-kind-of-file.md) (`same`/`chosen` outputs, multi-part jobs)

## Context

The app opens every file in a per-job sandbox and never goes online. That makes it a good place to answer two questions people rarely get answered before sharing a file: *what does this file give away?* and *is it safe to open?* Before this change the sniffer flagged macro-enabled Office files and nothing read the flag; PDF active content was reported but never removed; Office properties, audio and video tags and JPEG comments passed straight through conversions.

The product owner also asked for a few everyday conveniences: suggestions, contact sheets, folders, the clipboard and a command line.

## Decision

### Findings: what a file reveals

Inspect results gain `findings`, a list of `{kind, detail}` (at most 64, detail at most 200 characters). The kinds are `location`, `device`, `author`, `software`, `dates`, `comments`, `trackedChanges`, `hiddenContent`, `activeContent`, `macros`, `externalLinks`, `embeddedFiles` and `tags`.

- Privacy findings come from `converter_sdk/reveal.py`: EXIF/XMP/IPTC and comments for images; Info, XMP, annotations and hidden layers for PDF; `docProps`, comments, tracked changes and hidden sheets/slides for Office; metadata for SVG/HTML; container and stream tags for audio and video (from the ffprobe JSON the media inspect already reads).
- Active-content findings come from `converter_sdk/active.py`: PDF JavaScript, open actions, launch/submit/import actions, embedded files, rich media and XFA; Office macros, ActiveX, OLE objects, external relationships and DDE fields; SVG/HTML scripts, event handlers, script URLs, frames and external resources.
- A detail never contains document text. It names what was found ("Embedded comment", "3 custom properties") or a short metadata value (a camera model, coordinates, a date).
- The image, PDF and media inspectors return findings. `file-clean` inspects Office files, `file-disarm` SVG and HTML.

### Three new tools

| Tool (adapter) | Files | What it does | Fidelity |
|---|---|---|---|
| Remove hidden details (`file-clean`) | jpg, png, webp, tiff, avif, pdf, docx, xlsx, pptx | JPEG markers, PNG chunks and WebP chunks are dropped at byte level, so pixels don't change; TIFF and AVIF are re-encoded. PDF Info, XMP, piece info and thumbnails go, and optionally comments and hidden layers. Office properties are blanked and custom properties and comments removed. | E |
| Remove hidden details (`media-clean`) | audio and video | FFmpeg stream copy with all container, stream and chapter metadata dropped; cover art optionally. | E |
| Make safe to open (`file-disarm`) | pdf, docx, xlsx, pptx, svg, html | Removes everything that runs or loads something when the file is opened (see above). Forms with actions are flattened. Web links are kept unless asked. A macro-enabled `.docm` comes out as `.docx`. | H |

Each output is checked: `file-clean` and `media-clean` re-read their output and fail the job if location, device, author or dates remain. `file-disarm` runs an independent verifier in a fresh `python -I` process, like `pdf-redact`, and fails the job if anything active remains.

What they don't do is listed in [privacy.md](../privacy.md): a face or a street sign in a photo, text inside images, and tracked changes (reported, not accepted) all stay. Legacy `.doc`/`.xls`/`.ppt` aren't accepted: inspecting VBA in OLE files would need `oletools`, which isn't approved.

### Smart suggestions

`apps/desktop/src/suggestions.ts` offers up to three one-click picks from the selection, its findings and the available targets: make safe, remove location, HEIC to JPG, compress big photos, merge/combine/join several files of one kind, a video contact sheet, and a note when files are identical (the core's `duplicates` command compares SHA-256 hashes of same-size files). A suggestion only names a target the router reports as available.

### Contact sheets

`converter_sdk/sheets.py` lays thumbnails out in a captioned grid with Pillow's built-in font. `contact-sheet` makes PNG, JPG or PDF sheets from images and PDF pages; `video-frames` gains `layout: sheet`. No new engine.

### Folders and the clipboard

- A dropped or chosen folder is walked: no symlinks, no hidden or system files, eight levels at most, and still 200 files at most (the rest are reported as left out). Outputs carry the index of the input they came from (`OutputFile.source`), so a folder save can keep the folder layout. Every per-file tool sets it; the runtime fills it in when a job has one input; a result made from several inputs has none and goes in the top folder.
- A conversion can skip files already in the target format.
- Paste adds an image, a table (tab-separated text becomes CSV) or text as a file. The bytes are written to the data root's `pasted/` folder and removed when the selection is cleared or the app exits.
- Copy puts a picture or text result on the clipboard. A picture is decoded by its output's sandboxed worker, the same one that renders previews, into a PNG of at most 4096 pixels on its longest side; the core reads only that PNG. It uses the official `tauri-plugin-clipboard-manager` from Rust only; the renderer gets no clipboard permission and still never sees a path.

### Command line

`apps/cli` builds `lfcs`, which uses app-core directly: the same router, sandbox, checks and saving rules as the app. It has `doctor`, `inspect`, `targets`, `convert` and `run`, `--json` output and exit codes 0 (ok), 1 (job failed), 2 (usage), 3 (sandbox unavailable). It is its own sandbox runner (`--sandbox-run`). Locating Python, adapters and engines moved from the desktop's `main.rs` into `AppConfig::locate`, and schema defaults into `app_core::defaults`, so both front ends behave the same.

### Engines and dependencies

- `lxml` is declared as an approved engine. It was already installed (pikepdf depends on it) and used by `markup.py`. XML is parsed with entities, DTDs and network access off, and a DOCTYPE in an Office part or SVG is refused as corrupt.
- `tauri-plugin-clipboard-manager` is the only new dependency.

## Consequences

- Inspect does a little more work for every added file; the findings are read from structures the inspectors already open.
- Byte-level image cleaning has to track the container formats; tests compare decoded pixels before and after.
- The CLI ships in the workspace for now and is documented in its README; bundling it with the desktop app is later work.
