# Contributing

Thanks for helping. This is a privacy-first app, so a few rules are firmer than usual.

## Rules that don't bend

- **No network code.** No update checks, telemetry, crash reports or downloads, in the app or in a worker. See [Privacy](docs/privacy.md).
- **Every parser of untrusted input runs in the per-job sandbox.** The core never opens, parses or decodes a user's file itself. Don't add a fallback that runs an engine without the sandbox.
- **Don't log file names, document text or passwords.**
- **No new dependencies without a decision.** Open an issue first. A new engine needs its license reviewed against AGPL-3.0-only and an entry in `engines/engines.yaml`.
- **Licence:** the project is AGPL-3.0-only. Your contribution is under the same licence.
- **Report security problems privately**, not in an issue. See [SECURITY.md](SECURITY.md).

## Setting up

You need Rust (via rustup), Node.js 22 or newer, pnpm and uv. See the [README](README.md#prerequisites).

```bash
pnpm install
uv sync
```

Don't clone into OneDrive, Dropbox, iCloud Drive or Google Drive.

From a checkout, the app and `lfcs` need development mode: set `LFCS_DEV=1` (see the README).

## Checks

Run before you open a pull request:

```bash
pnpm check
```

It runs the generated-code check, type checks, frontend tests, `cargo fmt`, `clippy`, the Rust tests, the registry check, `ruff` and `pytest`. CI runs the same checks, split into lint, Rust tests and Python tests, on macOS, Windows and Linux, plus `cargo-deny` and `pip-audit`.

Some tests need FFmpeg, LibreOffice or (on Linux) bubblewrap. The README says how to install them. Where the program isn't installed, locally those tests skip; in CI (`CI=true`) they fail. A skipped test proves nothing, so say in your pull request which ones didn't run on your machine.

## Making a change

- **Keep a change small and focused.** Add or update tests with it.
- **Generated files** (`packages/protocol`, the adapter registry) come from `pnpm generate` and `pnpm registry`. Edit the source and regenerate; CI fails on a stale copy.
- **Adapters** live in `adapters/<name>/` with a manifest, a README and tests. The conformance kit in `workers/conformance/` runs on every adapter. Declare only the engines and warnings the adapter uses.
- **User-visible text** is plain and short. Every error code needs a text in `apps/desktop/src/text.ts`.
- **Docs:** if you change what the app does with files, update [Privacy](docs/privacy.md) and [Known limits](docs/known-limits.md) in the same pull request. Don't claim what a test doesn't prove.
- **Decisions** that change scope, add an engine or change the security model get an ADR in `docs/adr/` with the next free number.

## Pull requests

Describe what changed and why, and what you ran. Say which platforms you tested on. Pull requests need a green CI and a review from the code owner (`.github/CODEOWNERS`).
