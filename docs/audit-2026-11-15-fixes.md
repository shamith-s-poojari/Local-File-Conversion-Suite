# Fixes for the 2026-11-15 audit

The director (Opus) plans, reviews and integrates; Sonnet workers write the code. Each worker owns an exclusive list of files within its wave. No commits, pushes or new dependencies without the product owner's yes.

## Step 0: shared contract (director)

- `protocol.schema.json`: warning codes `ENCRYPTION_REMOVED` (P6) and `WARNINGS_TRUNCATED` (S4); bindings regenerated; texts in `apps/desktop/src/text.ts`.
- `apps/desktop/src/api.ts`: `SaveResult.path`, a display-only destination (U7, R7). Filled by C2.
- `JobStore::job_snapshot(&JobId)` (R10); app-core uses it.

## Work packages

| Wave | Package | Owns |
|---|---|---|
| 1 | A: sandbox and engine grants | `crates/sandbox/**`, `crates/app-core/src/config.rs`, `crates/app-core/src/engines.rs` |
| 1 | B: scheduler and jobstore | `crates/scheduler/**`, `crates/jobstore/**` |
| 1 | D1: privacy tools | `workers/sdk/src/converter_sdk/{active,reveal,verify}.py`, `adapters/{file-clean,media-clean,file-disarm}/**`, their tests |
| 1 | D2: SDK robustness, other adapters | `workers/sdk/src/converter_sdk/{images,pdf,engines,office,markup,convert,adapter}.py`, `workers/conformance/**`, `adapters/{pdf-compress,office-convert,pdf-redact,markup-convert,video-join,image-convert,pdf-protect,video-edit,audio-edit}/**` |
| 1 | E1: front end without backend changes | `apps/desktop/src/**` |
| 1 | F: CI and docs | `.github/**`, `docs/**` (not this file), `README.md`, new root files |
| 2 | C1: app-core | `crates/app-core/src/**` except `config.rs`, `engines.rs` |
| 2 | D3: redaction re-check (S10, ADR 0006) | decided after C1's job API settles |
| 3 | C2: CLI and Tauri shell | `apps/cli/**`, `apps/desktop/src-tauri/**` |
| 3 | E2: front end needing the backend | `apps/desktop/src/**` |

## Checklist

Ticked only after the director has read the diff and verified the fix.

### Security
- [x] S1 development mode opt-in; no working-directory search; interpreter inside repo and owned by user (A done and checked with a hostile `.venv` and `lfcs doctor`; `--dev` flag C2) — checked with a hostile `.venv` and `lfcs doctor`; `lfcs --dev`; debug desktop builds count as dev
- [x] S2 `install_root` allow-list, refuses home and `/`, ignores empty/relative PATH (A)
- [x] S3 bounded worker stdout queue, rate count in reader thread (B)
- [x] S4 64 warnings per job, de-duplicated by code (B)
- [x] S5 `installation_root` grants `Python.framework/Versions/X`, never `/Library` (A)
- [x] S6 macOS runaway descendants found and killed before verification (B)
- [~] S7 Linux: `--cap-drop ALL`, `--disable-userns` (bwrap ≥ 0.8), `RLIMIT_NPROC` cap done; seccomp deferred by the product owner (needs a new crate). Unverified on Linux (A)
- [x] S8 sandbox probe checks expected errno; new probes (A)
- [x] S9 close inherited fds ≥ 3 in `pre_exec` (B)
- [x] S10 independent re-check of pdf-redact and file-disarm output (D3, ADR 0006) — scheduler runs a sandboxed `output-check` job before success; a stand-in adapter that claims success on a leaky PDF ends failed
- [~] S11 O_NOFOLLOW / reparse-point-safe reads for clipboard, `read_output`, preview (C1) — `open_regular` in the core and preview handler (dev/ino-checked); O_NOFOLLOW values hard-coded per arch until `libc` is approved for app-core
- [x] S12 `check_output` rejects Windows device names; outputs are regular files (B)
- [x] S13 `RLIMIT_FSIZE` (16 GiB), a 1 s out/work size walk, and the SDK maps EFBIG to `E_RESOURCE_DISK` (B, D2)
- [x] S14 Windows ACEs: documented only (A, F) — documented in sandbox README, windows.rs, privacy.md and threat model
- [x] S15 name sanitiser: Default_Ignorable, superscript COM/LPT, CONIN$/CONOUT$ (B)
- [x] S16 `PendingSaves` one-time token, cancel, cleared on discard (C2; UI sends and declines with it, G)

### Rust correctness and maintainability
- [x] R1 no `files` lock during worker I/O, sniffing or download (C1)
- [x] R2 `with_output_worker` drops broken workers, takes worker out, sizes watchdog (C1)
- [x] R3 preview worker start failure fails and discards the job (C1)
- [x] R4 `shutdown` cancels and joins runners with a timeout (C1)
- [x] R5 `drive` panics become `E_INTERNAL` (B)
- [x] R6 close workers and drop layouts only after discard/save succeeds (C1)
- [x] R7 `save_outputs` returns `SaveReport`; CLI prints it (C1, C2) — core returns `SaveReport`; the CLI prints its paths; directory diffing gone
- [x] R8 CLI uses `intake::walk`, reports `folderTruncated` (C2)
- [x] R9 `python_modules` timeout reports no engines (A)
- [x] R10 `JobStore::job_snapshot` (director, step 0)
- [x] R11 `Partial` usage reported; director decides (B, director) — kept; the scheduler now produces it for multi-part jobs (`PART_FAILED` per failed part); memory retry reruns every part
- [x] R12 `sync_folder` removed or filled in (C1)
- [x] R13 `add_pasted` without the 50 MB copy or a lock during write (C1) — no copy in the core or the shell; no lock during the write
- [x] R14 preview folder tag uses the output index (C1)
- [x] R15 per-input passwords, or the limit documented (C1) — one password per part (protocol limit); differing passwords now fail early with `passwordsDiffer`
- [x] R16 order applied before grouping (C1)
- [x] R17 CLI Ctrl-C cancels and shuts down cleanly (C2) — CLI handles SIGINT/SIGTERM/console Ctrl-C without a new crate; test sends SIGINT mid-job
- [x] R18 `PendingSaves` cleanup (with S16) (C2)
- [x] R19 `Reason` enum with `selfTestPending` (C1)
- [x] R20 `saturating_add` (C1)

### Python and privacy tools
- [x] P1 linear CSS scanning, 64 KB cap; 1 MB in under 1 s (D1)
- [x] P2 file-clean: Office media EXIF, PDF DCT metadata, ZIP dates, external relationships, tracked changes; verifier extended (D1) — xlsx links to other workbooks are kept (formulas need them) and documented in privacy.md
- [x] P3 one `check_pixels` before every raster; preview dpi clamped (D2)
- [x] P4 EPUB-to-HTML output disarmed (D2)
- [x] P5 media-clean keeps attachment filename/mimetype, drops data streams; verifier (D1)
- [x] P6 `ENCRYPTION_REMOVED` from every PDF tool that drops encryption (D2) — contact-sheet from an encrypted PDF does not warn yet
- [x] P7 active content: connections, customXml, customUI, altChunk; one detector for PDF (D1, D2)
- [x] P8 frame count and frames × pixels caps (D2)
- [x] P9 file-clean uses `markup.check_zip` (D1)
- [x] P10 video-join concat list escapes `'` (D2)
- [x] P11 `MAX_PIXELS` is the real limit; Pillow matches; contact sheets fit (D2)
- [x] P12 SVG 1.0/1.1 DOCTYPE without internal subset accepted (D1)
- [x] P13 `word_parts()` / `deck_parts()` hoisted (D1)
- [x] P14 CFB-wrapped (password) Office gives `E_PASSWORD_REQUIRED` (D2)
- [x] P15 disarm before LibreOffice in office-convert, or documented (D2) — documented in office-convert README: disarm would drop objects the conversion keeps
- [x] P16 Application field blanked, not invented (D1)
- [x] P17 video-edit and audio-edit re-check param ranges (D2)
- [x] P18 engines get a minimal environment (D2)
- [x] Shared `converter_sdk/verify.py` used by file-disarm and pdf-redact (D1, D2)
- [x] SDK de-duplication: image-convert on `converter_sdk.images`, one `MB`, merged `issue()`, shared media `validate_input` (D2)
- [x] Conformance kit: hostile inputs, `suggestedName`, text sentinel, secret assertion, cancel tests, unused `fidelityNotes` (D2)

### Usability and accessibility
- [x] U1 ErrorBox looks up `reason` first (E2)
- [x] U2 `FILES_ADDED` handled in every state (E1)
- [x] U3 `engineMissing` shows the install hint (E2)
- [x] U4 E_LIMIT_EXCEEDED text rewritten (E1)
- [x] U5 per-reason texts, why/todo in FileRow (E2)
- [x] U6 picker errors shown (E1)
- [~] U7 saved screen shows destination and "Show in folder" (E2, C2) — to click in the built app
- [x] U8 per-file failures and warning details on "Partly done" (E2; file names from the core's staged input order, G)
- [x] U9 disabled Convert says what's missing; busy in preflight; protect needs password (E1)
- [~] U10 radio groups with roving tabindex (E1)
- [~] U11 focus `h1` on screen change; alerts on Failed/Result (E1)
- [x] U12 timer out of live region; staged announcements (E1)
- [x] U13 `--muted` contrast 4.5:1 (E1)
- [x] U14 PageOrganizer keys, focus, announcement, Reset (E1)
- [x] U15 passwords kept until `create_job` succeeds; confirm field (E1)
- [x] U16 `inspectAll` skips inspected files (E1)
- [~] U17 RedactionCanvas fixes (E1)
- [x] U18 shared code/warning text for the CLI, kept in sync by a test (C2)
- [x] U19 `--password-stdin`, `LFCS_PASSWORD` (C2)
- [x] U20 one `--json` error shape; `ok` in doctor (C2)
- [x] U21 `TIER_LABELS` deleted (E1)
- [x] U22 `createConversionSkipping` with "Skipped N" (E1)
- [x] U23 Settings plain names, IDs in a disclosure (E1)
- [x] U24 named checks (E1)
- [~] U25 dropzone highlight, reduced motion, font size (E1)
- [x] U26 removing the last file goes to idle (E1)
- [x] U27 SchemaForm field errors (E1)
- [x] U28 real `getTargets` error; no `eslint-disable` (E1)
- [~] U30 "Copy diagnostics" (E1)
- [x] `E_WRONG_STATE` text; screens split; component tests (static markup: no Testing Library installed); every `E_*` has text (E1)

`[~]`: code and helper tests done; focus, keyboard, drag events and clipboard still to check by hand in the built app.

U29 is not in the brief; nothing to do for it.

### CI, docs and release
- [x] Actions pinned to SHAs; cargo-fuzz pinned (F)
- [x] bubblewrap on Linux CI; `CI=true` makes skips fail; Linux `end_to_end` and `network` (F)
- [x] FFmpeg on all OSes, LibreOffice on Linux (F)
- [x] CI split into lint / rust-test / python-test with caches (F)
- [x] Dependabot (F)
- [x] `deny.toml`, `pip-audit`: need approval (F)
- [x] Docs reconciled with CI per OS; privacy.md list matches D1 (F)
- [x] ADR 0001 status, ADR 0002 gap; CONTRIBUTING, SECURITY, threat model, full-test README section, release plan (F)

## Deferred (needs approval or out-of-scope work)

- **Linux seccomp filter (S7).** Deferred by the product owner: it needs a new crate (`seccompiler` or similar). The other Linux hardening is in, but it has not run on Linux yet; CI's Linux job is its first run.
- **`libc` for app-core (S11).** `O_NOFOLLOW` values are hard-coded per OS and architecture, with a dev/ino check after opening. Adding `libc` (already in the lock file through other crates) as a direct app-core dependency would replace the constants; it needs approval.
- **CLI signal handling (R17)** declares `signal`, `_exit` and `SetConsoleCtrlHandler` by hand instead of adding `ctrlc` or `libc`. This works, but it is unsafe FFI that a reviewer should look at.
- **Windows ACE removal (S14).** Granted ACEs stay on folders after a crash. Removing them reliably needs the installer, so it is documented only.
- **Installers, signing, notarisation and updates.** These belong to the release plan (`docs/release-plan.md`), not to this audit.
- **contact-sheet from an encrypted PDF (P6)** does not warn `ENCRYPTION_REMOVED` yet. Its output is an image sheet, so the gap is in the notice, not in the protection.
- **`engineMissing` (U3)** shows the install hint, but the core doesn't say which engine is missing.

## Hand test (2026-11-17)

- CLI, macOS release build, `LFCS_DEV=1`. All of these were checked:
  - `doctor --json`: the sandbox probes pass, and `ok` is in the output. Without dev mode it fails with `E_INSTALL_CORRUPT`/`appNotStarted`.
  - `convert --json`: stdout is JSON only; progress goes to stderr.
  - `run file-clean`: the GPS location is gone from the photo.
  - `run file-disarm`: `.docm` becomes `.docx` with no `vbaProject`, and the result has a `MACROS_REMOVED` warning.
  - `run pdf-protect --password-stdin`: the output needs that password to open.
  - `--set password=…` is refused with `passwordInArgv`, exit code 2.
  - `--keep-folders`: the folder layout, `sub/` included, is kept.
- The release `.app` has no worker runtime yet (bundling it is installer work), and since S1 a release build never falls back to the checkout. It starts with processing off. Its Home banner used to say "Processing is turned off" with a hint about `LFCS_DEV`, which a release build ignores. It now says part of the app is missing, with the core's reason under "Details" (test in `screens.test.tsx`). Use a debug bundle (`tauri build --debug --bundles app`) for hand tests.
- The debug app bundle was built and launched, but access to drive it was declined in this session. The `[~]` items are still to check by hand in `Local File Conversion Suite.app`:
  - U7: the saved screen and "Show in folder";
  - U10, U11, U17 and U25: keyboard, focus, the redaction canvas and drag-and-drop;
  - U30: "Copy diagnostics";
  - quick previews while adding files.
