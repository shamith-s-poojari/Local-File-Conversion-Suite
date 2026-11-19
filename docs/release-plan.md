# Release plan: what stands between this and 1.0

A plan, not a promise. Several items need certificates or accounts that the project doesn't have. Parts of items 1, 2, 5 and 6 now exist as scripts and workflows (marked below), but none of them has run yet, so there is no installer, and the app runs only from a checkout in development mode (`LFCS_DEV=1`).

## Blockers for 1.0

### 1. A bundled runtime (spike S2)

Today the app uses the checkout's `.venv`. A release needs its own Python and engines, unpacked beside the app and started in isolated mode (`-I`). Spike S2 ([spec section 4](specs/phase-1.md)) is to show that:

- python-build-standalone with locked wheels works on each OS, including with macOS code signing, where nested binaries may need a different location in the bundle;
- one installer per OS installs on a clean machine and runs a merge offline;
- the real installer sizes are known, with and without FFmpeg and LibreOffice.

**Started:** `tools/bundle-runtime/bundle_runtime.py` builds that folder: a pinned CPython from python-build-standalone (installed by `uv`), the packages locked in `uv.lock` (hashes checked, wheels only), the adapters without their tests, and the engine catalog, laid out as `python/`, `adapters/` and `engines/`, which is where `crates/app-core/src/config.rs` looks in a packaged build. It also writes a Tauri config that adds them as bundle resources. The release app is its own sandbox runner (`--sandbox-run`), so no sidecar binary is needed. The runtime hasn't been built on any system yet, so none of S2's questions above is answered: sizes, macOS signing of the nested binaries, and a clean-machine run are all open. Until the worker libraries have their own dependency group in `pyproject.toml`, the script prunes the development tools (pytest, ruff, the code generator) by name from the `dev` group.

Open question for S2: whether FFmpeg and LibreOffice ship in the installer or stay user-installed, as now. Their licenses (GPL and MPL parts) and sizes decide it.

### 2. Installers

One installer per OS (Windows, macOS, Linux) built in CI from a tagged commit. **Started:** `.github/workflows/release.yml` builds an unsigned NSIS installer, DMG and `.deb` on a `v*` tag (or by hand), checks that the tag matches the manifests' version, runs the command line against the bundled runtime and against the resources inside the finished package (`tools/bundle-runtime/smoke.sh`), and uploads each system's files with a `SHA256SUMS`. It doesn't sign, create a release or publish, and it hasn't run. This also lets us:

- remove the Windows AppContainer access entries on uninstall;
- include AGPL notices and the source offer;
- run the sandbox self-test on a clean machine before release.

### 3. Signing and notarisation

- **macOS:** a Developer ID certificate, hardened runtime, every nested binary signed, notarisation and stapling.
- **Windows:** a code-signing certificate, to avoid SmartScreen warnings.
- **Linux:** signed packages or checksums, depending on the format chosen.

Signing keys live in CI secrets or a hardware token, never in the repository. The project has no certificates today; getting them is the first step.

### 4. Updates

The app has no network code, so updates are downloaded by hand from the release page, as [Privacy](privacy.md) says. A release needs:

- signed, versioned downloads with published checksums;
- a short guide to verify them;
- no in-app update check unless the product owner changes that rule, which would also change the privacy page and the network test.

### 5. Changelog and versioning

- Semantic versions; the version is `0.1.0` in `Cargo.toml`, `package.json` files and `pyproject.toml` files today, and a release sets every manifest that carries one together. CI fails if they differ (`tools/check-versions.py`), and the release workflow also checks the tag.
- A [`CHANGELOG.md`](../CHANGELOG.md) kept per release, in plain words: what changed for users, and what was fixed. It exists, with an Unreleased section.
- Release notes list known limits that changed.

### 6. Pinned engine versions with checksums

`engines/engines.yaml` has `version: null` for every engine. Before release each engine gets a version and a checksum for the exact build we ship, verified at build time, and the app reports the versions in diagnostics. Until then CI installs whatever the runner's package manager provides, and results can differ between runs. Every CI job prints the versions it got (`tools/engine-versions.py`), and [engines](engines.md) says where each is looked for.

### 7. Scrubbed logs

The app writes no logs with file names, text or passwords today. Before release:

- a test that runs every tool on files with marker names and text and fails if a marker appears in any log, diagnostic or error message;
- "Copy diagnostics" in the interface shows only what the user can read before copying, with paths replaced;
- crash handling that never includes file contents or paths.

## Also needed before 1.0

- The Windows sandbox confirmed on real machines, and the Linux sandbox on more than one distribution (it passes on GitHub's Ubuntu 24.04 runner).
- A network test on Windows: its socket sampler doesn't exist there.
- Spike S4 (WebView traffic) finished, and the packet-capture test passing on Linux (macOS passes nightly) and wired up on Windows.
- Release notes that say the Linux seccomp filter is a deny-list for x86-64 and AArch64 only.
- A clean-machine test of the installers on each OS.

## Out of scope here

Buying certificates and publishing releases. This page lists the work; the parts marked "Started" are in the repository and haven't run yet.
