# Local File Conversion Suite

A privacy-first file converter and PDF toolkit. All processing happens on your machine: no file or metadata is uploaded anywhere.

It converts between 41 formats: documents (PDF, Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, Markdown, HTML, text), data (CSV, JSON, YAML), images (PNG, JPG, WebP, HEIC, GIF, SVG, TIFF, AVIF and more), audio (MP3, WAV, FLAC, OGG, Opus, M4A, AAC, AIFF) and video (MP4, MOV, MKV, WebM, AVI). A selection of different kinds of file can be converted together, for example Word files and photos all to PDF. See [ADR 0003](docs/adr/0003-all-format-conversion.md).

It also has tools for each kind of file ([ADR 0004](docs/adr/0004-tools-for-every-kind-of-file.md)):

- **PDF:** merge, split, organize pages, compress, page numbers, protect or unlock, watermark, redact.
- **Images:** rotate and resize, compress to a size, make an animated GIF, put images into one PDF.
- **Audio:** join, and change the volume, fades or speed.
- **Video:** join, rotate or change speed, and save frames as pictures.
- **Spreadsheets:** combine CSV and Excel files, or split them by rows or by a column.

And, for every file you add ([ADR 0005](docs/adr/0005-safety-privacy-and-convenience.md)):

- **What it reveals:** location, camera, author, dates, comments, tracked changes, hidden content, and whether it can run macros or scripts or load things from outside, all found on your device.
- **Remove hidden details** from photos, PDFs, Office files, audio and video, without changing the pictures or re-encoding the sound.
- **Make safe to open:** remove macros, scripts, embedded files and outside links from PDFs, Office files, SVG and web pages, checked by an independent verifier.
- **Contact sheets** of photos, PDF pages or a video; **suggestions** for the selection; **folders** in and out with their layout kept; **paste** a picture or a table, and **copy** a result.
- **`lfcs`**, a command line that runs the same sandboxed tools. See [apps/cli](apps/cli/README.md).

**Status:** On macOS, every conversion and tool runs end to end in a per-job sandbox. The Windows and Linux builds compile and pass the lint and type checks, but their sandboxes haven't run on those systems yet. Audio and video use FFmpeg, and Office formats use LibreOffice, if they're installed; the app shows the install command when they aren't. There are no installers yet. See [known limits](docs/known-limits.md).

- [Architecture blueprint](docs/architecture/blueprint.md)
- [Phase 1 specification](docs/specs/phase-1.md) (wins wherever it differs from the blueprint)
- [Architecture decision records](docs/adr/)
- [Privacy](docs/privacy.md): what "processed on your device" means, and how it's tested
- [Known limits](docs/known-limits.md)

## Prerequisites

| Tool | Version | Purpose |
|---|---|---|
| Rust (via rustup) | see `rust-toolchain.toml` | Core and app shell |
| Node.js | 22 or newer | Frontend tooling |
| pnpm | see `packageManager` in `package.json` | JavaScript workspace |
| uv | latest | Python workspace; installs the pinned Python itself |

Platform requirements:

- **Windows:** Microsoft C++ Build Tools and WebView2
- **macOS:** Xcode Command Line Tools
- **Linux:** the packages listed in `.github/workflows/ci.yml`

**Don't clone this repo into OneDrive, Dropbox, iCloud Drive or Google Drive.** Test files and build output would sync to the cloud.

## Common commands

```bash
pnpm install
```

```bash
uv sync
```

```bash
pnpm check
```

```bash
pnpm dev
```

## Repository layout

| Path | Contents |
|---|---|
| `apps/desktop/` | React UI, and the Tauri shell in `src-tauri/` |
| `apps/cli/` | `lfcs`, the command line |
| `crates/` | Rust libraries, added as their tasks begin |
| `packages/protocol/` | Worker protocol schema, fixtures and code generator. See its [README](packages/protocol/README.md) |
| `workers/` | Adapter SDK and conformance kit |
| `adapters/` | One folder per adapter |
| `docs/` | Blueprint, specs, ADRs, spike reports |

## License

GNU Affero General Public License v3.0 only. See [LICENSE](LICENSE).
