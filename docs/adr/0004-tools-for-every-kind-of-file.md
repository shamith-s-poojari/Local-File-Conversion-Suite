# ADR 0004: Everyday tools for images, audio, video and data files, plus two more for PDFs

- Status: accepted
- Date: 2026-11-13
- Builds on: [ADR 0003](0003-all-format-conversion.md) (formats and engines); schedules `pdf-compress` from [ADR 0001](0001-requested-feature-list-scope.md)

## Context

After ADR 0003 the app converts between 41 formats, but only PDFs have tools: merge, split, organize, protect, watermark and redact. The product owner asked for the same kind of help with other files, as long as it doesn't overcomplicate the app: rotate and compress photos, join recordings, cut a long spreadsheet into parts.

The conversions already offer some editing: image quality, size and EXIF removal; audio bitrate, sample rate, channels and trimming; video quality, resolution, muting, trimming and GIF output. The new tools cover what a conversion can't do.

## Decision

### Twelve tools, one adapter each

| Files | Tool (adapter) | What it does | Output |
|---|---|---|---|
| Images | Rotate, flip or resize (`image-edit`) | Quarter turns, mirror, resize by percentage or to fit, black and white, remove camera details | Each file in its own format |
| Images | Compress (`image-compress`) | Under 100 KB to 2 MB: quality first, then colours (PNG), then size | Each file in its own format |
| Images | Animated GIF (`images-to-gif`) | Pictures in the chosen order, frame time, width, loop | One GIF |
| Audio | Join (`audio-join`) | In the chosen order, with optional silence between files | One file, format chosen |
| Audio | Volume, fade and speed (`audio-edit`) | EBU R128 loudness, ±dB, fades, 0.5–2× with the pitch kept | Each file in its own format |
| Video | Join (`video-join`) | Stream copy when clips match, else fitted to the first clip and re-encoded | One file, format chosen |
| Video | Rotate and speed (`video-edit`) | Quarter turns, mirror, 0.25–4×, mute | Each file in its own format |
| Video | Save frames (`video-frames`) | A number spread evenly, or one every few seconds | Pictures, JPG or PNG chosen |
| Data | Combine (`data-combine`) | CSV and Excel: rows stacked with columns matched by name, or a sheet per file | One file, CSV or Excel chosen |
| Data | Split (`data-split`) | Every N rows, or one file per value in a column, with the header repeated | Each file in its own format |
| PDF | Compress (`pdf-compress`) | Pictures resampled to 150 or 96 dpi and re-encoded as JPEG, then the file is repacked; or lossless repacking only. The original is kept if nothing shrinks | One PDF |
| PDF | Page numbers (`pdf-page-numbers`) | Six positions, four styles, optional text, start number, chosen pages | One PDF |

Each tool has a plain form built from its parameter schema; no new widget was needed. Joins and the GIF maker reuse `fileOrder`. Every engine was already approved: Pillow, FFmpeg, openpyxl, pikepdf and ReportLab.

### Tools that keep each file's format

Until now an operation's output format was fixed in its manifest (`outputs.format: pdf`), and the scheduler checked every output against that one format. Two values were added:

- **`same`**: each output has its input's format. The core groups the selection by format and runs **one part per format** in one job, using the multi-part jobs from ADR 0003. A JPG and a PNG rotated together give a JPG and a PNG.
- **`chosen`**: the format in the `to` param. The registry check refuses a manifest whose schema doesn't list `to` as an enum of formats.

To support this, the output format moved from the job (`JobSpec`) to each part (`JobPart`). The runner checks every output against its own part's format after the last worker has exited, as it already did for one format, so `E_OUTPUT_INVALID` still means a file isn't what its tool promised.

The form shows a tool's `to` field as an ordinary choice ("Save as"). A conversion's `to` stays out of the form, because the target picker sets it.

### Shared code

The new adapters share three SDK modules rather than copy code between adapters:

- `converter_sdk.images`: opening images with the decompression-bomb guard, upright frames, and writing each format back. A TIFF's own tag directory is never copied into a new file, since that corrupts it.
- `converter_sdk.tables`: CSV and Excel reading and writing under the same limits and number rules as data-convert, which now uses it too.
- `converter_sdk.media`: the encoder and container tables video-convert already used (now imported from here), plus `atempo` chains, frame rates and extra-stream detection.

Page placement (`displayed_box`, `placement`) moved from pdf-watermark into `converter_sdk.pdf`, so page numbers sit where the viewer sees them on rotated and cropped pages.

### New warnings

| Code | When |
|---|---|
| `REENCODED` | Clips didn't match, so the join re-encoded them |
| `KEPT_ORIGINAL` | A file was already small enough, or couldn't be made smaller, so it was kept byte for byte |
| `TARGET_NOT_REACHED` | An image couldn't get under the chosen size before reaching 64 pixels |
| `COLUMNS_MISMATCHED` | Combined tables had different columns; the gaps are empty |

## Consequences

- The home screen lists each family's tools on its tile, and the PDF tile gains Compress and Page numbers.
- Every tool runs in the per-job sandbox with the same output checks as the conversions. The end-to-end test runs each one on macOS.
- Speed changes, video edits and mismatched video joins re-encode, which takes time on long videos. The form says so.
- Page numbers use Helvetica, which every viewer has, so nothing is embedded. As with the watermark, only Western European characters show; others become "?".
- The image tools don't take HEIC (Pillow can't write it back), SVG or ICO; convert those first.
- Still not built: drawn or image signatures, image watermarks, crop with a pointer, and PDF/A. They need a placement widget or a validator.
