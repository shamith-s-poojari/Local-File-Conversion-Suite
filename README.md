# Local File Conversion Suite

A privacy-first file converter and PDF toolkit. All processing happens on your machine: no file or metadata is uploaded anywhere.

**Status:** Phase 1, week 1. The app launches but has no tools yet.

- [Architecture blueprint](docs/architecture/blueprint.md)
- [Phase 1 specification](docs/specs/phase-1.md) (wins wherever it differs from the blueprint)
- [Architecture decision records](docs/adr/)

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
| `crates/` | Rust libraries, added as their tasks begin |
| `packages/protocol/` | Worker protocol schemas (task T02) |
| `workers/` | Adapter SDK and conformance kit |
| `adapters/` | One folder per adapter |
| `docs/` | Blueprint, specs, ADRs, spike reports |

## License

GNU Affero General Public License v3.0 only. See [LICENSE](LICENSE).
