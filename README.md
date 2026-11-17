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

**Status:** On macOS, every conversion and tool runs end to end in a per-job sandbox. CI runs the same tests on Windows and Linux, but the Linux sandbox tests have never passed, and the Windows sandbox is unconfirmed on real machines. See [known limits](docs/known-limits.md#what-ci-proves). Audio and video use FFmpeg, and Office formats use LibreOffice, if they're installed; the app shows the install command when they aren't. There are no installers yet, so the app runs only from a checkout in development mode (below). See [known limits](docs/known-limits.md).

- [Architecture blueprint](docs/architecture/blueprint.md)
- [Phase 1 specification](docs/specs/phase-1.md) (wins wherever it differs from the blueprint)
- [Architecture decision records](docs/adr/)
- [Privacy](docs/privacy.md): what "processed on your device" means, and how it's tested
- [Known limits](docs/known-limits.md)
- [Threat model](docs/threat-model.md)
- [Release plan](docs/release-plan.md): what stands between this and a 1.0
- [Contributing](CONTRIBUTING.md) and [security reports](SECURITY.md)

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

## Development mode

There is no bundled runtime yet, so the app and `lfcs` use this checkout's `.venv` as the Python runtime. That is opt-in. Set `LFCS_DEV=1`, or pass `lfcs --dev` (coming; not available yet), when running from a checkout:

```bash
LFCS_DEV=1 pnpm dev
```

```bash
LFCS_DEV=1 cargo run -p lfcs -- doctor
```

Without it the core doesn't search the working directory for a runtime, so a hostile folder can't supply one. The core also refuses a development runtime that isn't inside the repository or isn't owned by the current user.

## Running the full tests

`pnpm check` runs everything that needs no outside program: the generated-code check, type checks, frontend tests, `cargo fmt`, `clippy`, the Rust tests, the adapter registry check, `ruff` and `pytest`. Some tests start programs that aren't part of the repository, and skip when they aren't there. Install them to run the whole suite (CI does, and with `CI=true` those tests fail instead of skipping):

- **FFmpeg**: audio and video tests. `brew install ffmpeg`, `sudo apt-get install ffmpeg` or `choco install ffmpeg`.
- **LibreOffice**: Office conversion tests. `brew install --cask libreoffice` or `sudo apt-get install libreoffice-core libreoffice-writer libreoffice-calc libreoffice-impress`. Run its tests one at a time, not alongside other test runs: `uv run pytest adapters/office-convert`, then the end-to-end test alone, with `cargo test -p converter-app-core --test end_to_end`.
- **bubblewrap** (Linux only): the sandbox. `sudo apt-get install bubblewrap`. On Ubuntu 24.04 and later, AppArmor blocks unprivileged user namespaces; as a test machine only, run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`. The sandbox, command-line, end-to-end and network tests need it. Without it the app turns processing off.
- **strace and tcpdump** (Linux, only for `tools/network-test/capture.sh`).

The Rust tests that run the workers need `uv sync` first, and `LFCS_DEV=1` if you run `lfcs` yourself.

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
