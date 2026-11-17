# Release plan: what stands between this and 1.0

A plan, not a promise. Nothing here is built yet, and several items need certificates or accounts that the project doesn't have. Until they are done there is no installer, and the app runs only from a checkout in development mode (`LFCS_DEV=1`).

## Blockers for 1.0

### 1. A bundled runtime (spike S2)

Today the app uses the checkout's `.venv`. A release needs its own Python and engines, unpacked beside the app and started in isolated mode (`-I`). Spike S2 ([spec section 4](specs/phase-1.md)) is to show that:

- python-build-standalone with locked wheels works on each OS, including with macOS code signing, where nested binaries may need a different location in the bundle;
- one installer per OS installs on a clean machine and runs a merge offline;
- the real installer sizes are known, with and without FFmpeg and LibreOffice.

Open question for S2: whether FFmpeg and LibreOffice ship in the installer or stay user-installed, as now. Their licenses (GPL and MPL parts) and sizes decide it.

### 2. Installers

One installer per OS (Windows, macOS, Linux) built in CI from a tagged commit. This also lets us:

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

- Semantic versions; the version is `0.1.0` in `Cargo.toml` and `pyproject.toml` today, and a release sets every manifest that carries one together.
- A `CHANGELOG.md` kept per release, in plain words: what changed for users, and what was fixed.
- Release notes list known limits that changed.

### 6. Pinned engine versions with checksums

`engines/engines.yaml` has `version: null` for every engine. Before release each engine gets a version and a checksum for the exact build we ship, verified at build time, and the app reports the versions in diagnostics. Until then CI installs whatever the runner's package manager provides, and results can differ between runs.

### 7. Scrubbed logs

The app writes no logs with file names, text or passwords today. Before release:

- a test that runs every tool on files with marker names and text and fails if a marker appears in any log, diagnostic or error message;
- "Copy diagnostics" in the interface shows only what the user can read before copying, with paths replaced;
- crash handling that never includes file contents or paths.

## Also needed before 1.0

- The Linux sandbox tests green in CI, and the Windows sandbox confirmed on real machines.
- Spike S4 (WebView traffic) finished, and the packet-capture test run.
- A Linux system-call (seccomp) filter decided: add it, or document its absence in the release notes. It needs a new crate.
- A clean-machine test of the installers on each OS.

## Out of scope here

Building installers, buying certificates and publishing releases. This page only lists the work.
