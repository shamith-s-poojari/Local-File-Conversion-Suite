# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/). Nothing has been released yet; there are no installers (see the [release plan](docs/release-plan.md)).

## [Unreleased]

The work of pull request #6, "Fix the 2026-11-15 audit findings", and the second audit that followed ([audit-2026-11-17](docs/audit-2026-11-17.md)). Parts of the second audit are still being fixed; this list covers what is in the branch's history.

### Added

- `lfcs`, the command line, runs the same sandboxed tools as the app: `doctor`, `inspect`, `targets`, `convert` and `run`. Folders go through the same walk as the app's, Ctrl-C and SIGTERM stop the job and clean up, and passwords come only from `--password-stdin` or `LFCS_PASSWORD`. `--dev` and `--resources` choose the runtime.
- An independent re-check for redaction and "make safe to open": before such a job can succeed, a separate sandboxed job re-reads copies of its outputs, and a failed or missing check fails the job and deletes its outputs ([ADR 0006](docs/adr/0006-independent-output-recheck.md)).
- Linux: a seccomp deny-list through bubblewrap (x86-64 and AArch64), a process cap, `--cap-drop ALL`, and a block on nested user namespaces (bubblewrap 0.8 and newer).
- macOS: a cap on the number of processes a worker may start. Workers carry a `LFCS_JOB` tag so processes left behind by a job can be found and stopped.
- Remove hidden details covers more: Office author, path and revision details, custom XML, printer settings, embedded files, `people.xml` and document variables; cover-art metadata; PNG and WebP chunk allow-lists and neutral ICC profile names; PDF annotation names and dates, and the file ID. Files embedded in Office documents are always removed; what they show on the page stays.
- Redaction removes the text under a box from the document's tags, alternative text, bookmarks, page labels and destinations too, and the verifier checks those places.
- Keyboard redaction by line, word and term; a page-text line list.
- CI helpers: `tools/check-versions.py` (every manifest has the same version), `tools/check-skips.py` (a ceiling on skipped Python tests), `tools/engine-versions.py` (FFmpeg and LibreOffice versions in each job's log).
- `tools/bundle-runtime`, which builds the worker runtime a release app would carry, and `.github/workflows/release.yml`, which builds unsigned installers from it. Neither has run yet.
- Documents: [troubleshooting](docs/troubleshooting.md), [engines](docs/engines.md), the [threat model](docs/threat-model.md), the [release plan](docs/release-plan.md), [CONTRIBUTING](CONTRIBUTING.md) and [SECURITY](SECURITY.md).
- CI: lint, Rust tests and Python tests on Windows, macOS and Linux, `cargo-deny`, `pip-audit`, Dependabot, and nightly fuzzing and packet-capture jobs. Actions are pinned to commit SHAs. Clippy runs on all three systems, jobs have timeouts, and LibreOffice is installed on all three systems.

### Changed

- Development mode is opt-in. The app and `lfcs` use a checkout's `.venv` only with `LFCS_DEV=1` or `--dev`; the working directory is never searched, and the interpreter must be inside the checkout and owned by the current user. (A debug build of the desktop app is built from a checkout and uses it.)
- Release builds keep unwinding on panic, so a caught panic fails one job instead of closing the app.
- Verified outputs are hash-checked when saved; the disk limit covers the whole job folder; a failed retry cleanup fails closed; events are delivered outside the store lock.
- Active-content detection covers connections, custom XML, custom UI and altChunk; SVG 1.x document types are accepted; CSS scanning is linear and capped at 64 KB.
- The interface: errors say what to do per reason; the disabled Convert button says what is missing; files can be dropped in any state; radio groups use roving tab index and focus moves to each screen's heading; passwords are kept until the job starts, with a confirm field; "Partly done" names the failed files; the saved screen shows the destination and "Show in folder"; reduced motion is respected; text and control contrast raised; a neutral state while the self-test is still running; confirmation before Start over.
- A damaged input maps to `E_INPUT_CORRUPT` or `E_LIMIT_EXCEEDED`, not a retryable `E_INTERNAL`.
- Pillow is 13.0.0, for its security advisories.

### Fixed

- Linux: bubblewrap refused `--disable-userns` without `--unshare-user`, so every sandboxed run failed; the interpreter behind uv's links is now found inside the sandbox; `/etc/alternatives` is visible, so FFmpeg loads; the self-test accepts bubblewrap's home-folder skeleton.
- LibreOffice exits with code 81 after it creates a profile and asks to be started again; the app runs the binary directly, so every Office conversion on Linux ended with no output. The worker now starts it once more.
- Windows: Chocolatey's FFmpeg is found instead of its shim; the self-test accepts the Job Object's refusal to start a shell.
- Ogg (Vorbis) output needs an FFmpeg with libvorbis. Without it the job fails with `E_INSTALL_CORRUPT` and the app looks for Homebrew's `ffmpeg-full` first among the fixed folders.
- The nightly network test on Linux failed because `strace` couldn't write its trace into a root-owned folder.

### Known limits

See [known limits](docs/known-limits.md).
