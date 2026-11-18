# Known limits

What the app can't do, or does only partly. Each tool's README has the details; this page collects them for users and reviewers.

## Platforms

| | macOS | Linux | Windows |
|---|---|---|---|
| Sandbox | Seatbelt | bubblewrap, with a seccomp deny-list (x86-64 and AArch64 only) | AppContainer and a Job Object |
| Sandbox tests in CI | Pass | Pass (2026-11-18); need user namespaces, see [troubleshooting](troubleshooting.md#linux) | Pass on a hosted runner; not confirmed on real machines |
| Office conversions (LibreOffice) in CI | Installed by the CI workflow; that change hasn't run yet | Run, and pass | Installed by the CI workflow; that change hasn't run yet |
| FFmpeg, LibreOffice | Homebrew and `/Applications` | The usual package paths | WinGet, Scoop, Chocolatey and `Program Files` |
| Installer | None built | None built | None built |

Where each engine is looked for is on the [engines](engines.md) page.

There is no installer yet, so the app only runs from a development checkout, in development mode: set `LFCS_DEV=1` or pass `lfcs --dev` (the core doesn't search the working directory for a runtime). `tools/bundle-runtime` builds the runtime folder a release app would carry, and `.github/workflows/release.yml` builds unsigned installers with it; neither has been run yet. Signing and notarization (REL-3) are not done. See the [release plan](release-plan.md). FFmpeg and LibreOffice aren't bundled: conversions that need them show as unavailable, with the install command, until they're installed.

If the sandbox self-test fails on a system, processing stays off there. The app never runs a conversion unsandboxed.

### What CI proves

CI (`.github/workflows/ci.yml`) runs on all three systems with `CI=true`. Under it the sandbox, command-line, end-to-end and network tests, and the FFmpeg tests, fail instead of skipping. Python tests that skip anyway are counted: a run fails if it skips more than a recorded ceiling (`tools/check-skips.py`), which is how a runner whose FFmpeg or LibreOffice failed to install is noticed. Clippy runs on all three systems. Each job prints the FFmpeg and LibreOffice versions it used; they aren't pinned. Results below are from the run of 2026-11-18 on commit `7c93f30`, except where a different run is named; check the latest run before relying on them.

- **macOS:** the sandbox, end-to-end and network tests pass. LibreOffice is now installed in CI, but the Office tests haven't run there yet: the run above was before that change.
- **Linux:** the sandbox, seccomp, stray-process, command-line, end-to-end and network tests pass, and so do LibreOffice conversions inside the sandbox (24.2.7, from Ubuntu's packages). That is one run on GitHub's Ubuntu 24.04 with user namespaces opened up. It says nothing about other distributions, where bubblewrap or user namespaces may be blocked; the app then turns processing off.
- **Windows:** the Rust and Python tests passed on a hosted Windows runner (run 44252855383, 2026-11-18; the run after it was cancelled on Windows). That includes the AppContainer sandbox tests. It has not been confirmed on a real machine, and LibreOffice inside the AppContainer has not been tried: CI now installs LibreOffice on Windows, so the first run of the Office tests there may show problems.
- **Network test on Windows:** the canary test's socket sampler reads `lsof` and the process tree, which exist on macOS and Linux only. On Windows `network.rs` prints "skipped" and exits successfully, even under `CI=true`, so the network test proves nothing on Windows. No packet capture is wired up for Windows either (`pktmon` is the plan).
- **Packet capture** (`tools/network-test/capture.sh`, nightly): it has passed on macOS in the nightly job (for example 2026-11-17). The Linux nightly failed on 2026-11-15, 16 and 17 because `strace` couldn't write its trace file; the script is fixed, and it hasn't run since. It has never been run with `--app`.

CI doesn't test installers, signing, updates or real-world machines, and it doesn't measure WebView traffic.

## Files

- **Limits:** 5,000 pages per file and 200 files per selection.
- **One open password per job.** Files with different open passwords must be unlocked one at a time first.
- **Online-only files** (OneDrive, iCloud Drive and similar) are downloaded only after you agree.
- A job cancelled at the moment it is created may stay in the store as cancelled.
- **Repaired files:** a damaged PDF that can be repaired on opening may still be refused if the damage survives the rewrite (`E_INPUT_CORRUPT`).

## Conversions

- **Mixed selections** (Word files and photos to PDF, songs and videos to MP3) use each kind's usual settings. Convert one kind at a time to change them.
- **Scanned PDFs** have no text to extract. Converting one to text, Markdown, HTML or Excel fails with `E_NO_TEXT`; Word or images keep the page images.
- **Best-effort conversions** (PDF to Word, PowerPoint or Excel, among others) rebuild structure from the page layout and are hidden until you ask for them. Check the result.
- **Data files:** values inside values (a list in a JSON record) become JSON text in a spreadsheet cell; Excel formulas become their last saved values; YAML comments are dropped. Excel to Markdown isn't offered.
- **Audio and video:** subtitles, data tracks and every audio track after the first are dropped when converting video (`STREAMS_DROPPED`). Converting to a lossy format (MP3, AAC, Opus, WebM) re-encodes, which loses a little quality each time.
- **Ogg (Vorbis)** needs an FFmpeg with libvorbis. Homebrew's `ffmpeg` 9 leaves it out; use `ffmpeg-full`, which the app finds when started from the Finder or when no plain `ffmpeg` is earlier on `PATH` (see [engines](engines.md#ffmpeg)). Without it, making an Ogg file fails with `E_INSTALL_CORRUPT`. FFmpeg's own Vorbis encoder isn't used: it crashes re-encoding Vorbis audio.
- **Office documents** depend on the installed LibreOffice: fonts the system lacks are substituted, and complex layouts may shift.

## Tools for images, audio, video and data

- **Image tools** take PNG, JPG, WebP, GIF, BMP, TIFF and AVIF. Convert HEIC, SVG and ICO first. Editing a JPG, lossy WebP or AVIF re-encodes it at high quality.
- **Compress images** aims for the size you choose but stops at 64 pixels on the shorter side (`TARGET_NOT_REACHED`). Camera details are always removed, and animated WebP keeps only its first frame.
- **Join audio** drops tags, cover art and extra tracks. Every file is resampled to the highest sample rate among them, at most 48 kHz.
- **Join video** copies matching clips without re-encoding. Clips that differ are fitted into the first clip's frame and re-encoded, which takes as long as converting them (`REENCODED`).
- **Rotate or change speed** re-encodes the whole video, and so does changing audio speed or volume.
- **Combine spreadsheets** stacks every sheet of a workbook; empty sheets are skipped. **Split** uses a workbook's first sheet only, and makes at most 1,000 files from one input.

## Safety, privacy and convenience (ADR 0005)

- **Remove hidden details** doesn't touch pixels, text inside pictures, or tracked changes (`TRACKED_CHANGES_KEPT`). A JPEG with a rotation in its EXIF, and TIFF and AVIF files, are re-encoded once (`REENCODED`); an animated AVIF keeps its first frame. Signed PDFs lose their signature (`SIGNATURE_INVALIDATED`).
- **Remove hidden details** always removes files embedded in Office documents (the workbook behind a chart, an inserted file); what they show on the page stays. JPEG ICC profile names and image attachments in Matroska files are not cleaned yet.
- **Make safe to open** keeps what you see but removes interaction: forms with actions are flattened, link annotations whose action was removed stay as inert links, and formulas that used an external workbook keep their last values. Excel 4.0 macro sheets are detected but not removed, so such a file fails the check rather than passing. Legacy `.doc`, `.xls` and `.ppt` aren't accepted.
- **Copy** puts any picture result (as an image, at most 4096 pixels on its longest side) or text result on the clipboard. Audio, video, PDF and Office results can't be copied; save them instead.
- **Folders:** a folder is read up to eight levels deep, without following links or reading hidden files, and still up to 200 files. **Keep folder layout** puts each result next to where its input was; a result made from several files (a merge, a join, a contact sheet) goes in the top folder.
- **Duplicates** are found by comparing file contents, so two copies of a photo with different metadata count as different.

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
- Under a box, words of three or more characters are tracked. A common short word in a term can also remove a bookmark or label that contains it.
- Verification is reported as **"passed these checks"**, not a guarantee. If checks fail and you accept the fallback, affected pages become images and their text is lost.
- Boxes over pictures and other non-text areas need a pointer. Terms, words, lines and whole pages work by keyboard.

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
- The packet-capture network test (`tools/network-test/capture.sh`) runs nightly and has passed on macOS; the Linux run is fixed but hasn't run since. There is no network test on Windows (see What CI proves).
- Linux has a small seccomp deny-list (ptrace, mounts, namespaces, kernel modules, BPF and similar, refused with an error). It is built for x86-64 and AArch64 only; on another architecture the namespaces are all there is. Other calls are allowed.
- macOS: after each job the scheduler looks for processes tagged with the job's id and stops them, so a worker can't leave a background process behind. Apple's own binaries hide their environment, so one of those started by a worker can't be found that way (Python workers are found). The process limit on macOS is a guard against fork bombs on your total, not a count per job.
- Windows AppContainer permission entries on the app's folders aren't removed on uninstall, because there is no installer yet.
- See the [threat model](threat-model.md) for what the sandbox does and doesn't cover.
