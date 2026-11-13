# Known limits

What the app can't do, or does only partly. Each tool's README has the details; this page collects them for users and reviewers.

## Platforms

| | macOS | Linux | Windows |
|---|---|---|---|
| Sandbox | Works (Seatbelt) | Written (bubblewrap), not yet run on Linux | Written (AppContainer and Job Object), compile-checked only |
| FFmpeg, LibreOffice | Found in Homebrew and `/Applications` | Found in the usual package paths | Found in WinGet, Scoop, Chocolatey and `Program Files` |
| Installer | Not built yet | Not built yet | Not built yet |

Packaging, signing and notarization (T21) are not done, so the app only runs from a development checkout. FFmpeg and LibreOffice aren't bundled: conversions that need them show as unavailable, with the install command, until they're installed.

If the sandbox self-test fails on a system, processing stays off there. The app never runs a conversion unsandboxed.

## Files

- **Limits:** 5,000 pages per file and 200 files per selection.
- **One open password per job.** Files with different open passwords must be unlocked one at a time first.
- **Online-only files** (OneDrive, iCloud Drive and similar) are downloaded only after you agree.
- **Repaired files:** a damaged PDF that can be repaired on opening may still be refused if the damage survives the rewrite (`E_INPUT_CORRUPT`).

## Conversions

- **Mixed selections** (Word files and photos to PDF, songs and videos to MP3) use each kind's usual settings. Convert one kind at a time to change them.
- **Scanned PDFs** have no text to extract. Converting one to text, Markdown, HTML or Excel fails with `E_NO_TEXT`; Word or images keep the page images.
- **Best-effort conversions** (PDF to Word, PowerPoint or Excel, among others) rebuild structure from the page layout and are hidden until you ask for them. Check the result.
- **Data files:** values inside values (a list in a JSON record) become JSON text in a spreadsheet cell; Excel formulas become their last saved values; YAML comments are dropped. Excel to Markdown isn't offered.
- **Audio and video:** subtitles, data tracks and every audio track after the first are dropped when converting video (`STREAMS_DROPPED`). Converting to a lossy format (MP3, AAC, Opus, WebM) re-encodes, which loses a little quality each time.
- **Office documents** depend on the installed LibreOffice: fonts the system lacks are substituted, and complex layouts may shift.

## Tools for images, audio, video and data

- **Image tools** take PNG, JPG, WebP, GIF, BMP, TIFF and AVIF. Convert HEIC, SVG and ICO first. Editing a JPG, lossy WebP or AVIF re-encodes it at high quality.
- **Compress images** aims for the size you choose but stops at 64 pixels on the shorter side (`TARGET_NOT_REACHED`). Camera details are always removed, and animated WebP keeps only its first frame.
- **Join audio** drops tags, cover art and extra tracks. Every file is resampled to the highest sample rate among them, at most 48 kHz.
- **Join video** copies matching clips without re-encoding. Clips that differ are fitted into the first clip's frame and re-encoded, which takes as long as converting them (`REENCODED`).
- **Rotate or change speed** re-encodes the whole video, and so does changing audio speed or volume.
- **Combine spreadsheets** stacks every sheet of a workbook; empty sheets are skipped. **Split** uses a workbook's first sheet only, and makes at most 1,000 files from one input.

## PDF compression and page numbers

- **Compress PDF** shrinks only 8-bit grey or colour pictures. CMYK, palette and spot-colour pictures, 1-bit scans, and JBIG2, CCITT or JPEG 2000 data are left alone, so some files barely shrink. If nothing gets smaller, the original is kept (`KEPT_ORIGINAL`).
- **Page numbers** show Western European characters only; others appear as "?" (`GLYPH_UNSUPPORTED`).

## Tools

**Merge**
- Accessibility tags aren't merged (`TAGS_DROPPED`).
- XMP metadata isn't carried over; only the basic document properties are.
- Form fields whose names clash with an earlier file's are flattened (`FORMS_FLATTENED`).
- Per-file bookmarks use the original file names.

**Split**
- Form fields are always flattened, because a field can't be split across files.
- "Maximum size" is best effort: a single page larger than the limit gets a part of its own.

**Organize**
- If any page is deleted or duplicated, form fields are flattened.

**Protect or unlock**
- Permissions such as "no printing" are honored voluntarily by PDF viewers. They are not access control.
- Lost passwords can't be recovered. The app never guesses or cracks passwords.

**Watermark**
- Text only; image watermarks aren't available yet.
- Until the bundled font ships with packaging, only Western European characters can be shown. Others appear as "?" (`GLYPH_UNSUPPORTED`).
- A watermark is a label. It can be removed by anyone with a PDF editor.

**Redact**
- Search can't find text inside scanned images. Pages with images and no text raise `NO_TEXT_LAYER`. Drawing boxes still works on scans.
- The size of a box can hint at the length of what it hides.
- Embedded font subsets can reveal which characters a document used, though not where.
- Content that is meaningful elsewhere, such as a name inside a chart image, isn't detected.
- Redaction also removes all comments, form fields, attachments, hidden layers, document properties, and bookmarks that contain a term.
- Verification is reported as **"passed these checks"**, not a guarantee. If checks fail and you accept the fallback, affected pages become images and their text is lost.
- Drawing boxes needs a pointer. Terms and whole-page redaction work by keyboard.

## Interface

- Previews are images rendered in the sandbox. Text in the preview can't be selected or searched.
- The page organizer loads thumbnails lazily but lists every page. Very long documents (thousands of pages) are slow to scroll.
- The redaction canvas's match list covers the first 500 pages; terms are still applied to every page.

## Error codes

The catalog has no code for an invalid parameter or a precondition, such as "the file is already encrypted". These come back as `E_INTERNAL` with the parameter name and reason, and the interface shows them next to the field. A catalog code is planned.

## Privacy and security

- Deleting a job's files is ordinary deletion, not secure erasure. See [Privacy](privacy.md).
- WebView network traffic isn't measured yet (spike S4).
- Sync-folder detection is best effort.
- The packet-capture network test (`tools/network-test/capture.sh`) hasn't run yet; the canary test has, on macOS.
