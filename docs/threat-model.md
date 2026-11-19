# Threat model

What the app protects, from whom, where the boundaries are, and what is not covered. It describes the code as it is; where something is unproven it says so. See [Privacy](privacy.md) for what users are told, and [Known limits](known-limits.md) for the rest.

## Assets

- **The user's files and their contents**, and the metadata in them (location, author, dates).
- **Passwords** the user types for protected files.
- **The rest of the user's computer:** their other files, their accounts and their network.
- **The output folder:** results must go only where the user chose.

## Adversaries

| Adversary | What they can do | What we want |
|---|---|---|
| **Hostile input file** (opened by the user from email or the web) | Craft a PDF, Office, image, audio, video, SVG or archive to exploit a parser, exhaust memory or time, follow links, run macros or scripts, or phone home | The file can't read other files, reach the network, or leave anything running; limits stop decompression bombs and runaway jobs |
| **Other local users or programs** | Read or replace the app's temporary files, plant a runtime or engine in a place the app searches, race the app on paths | Job folders are private; the app only runs a runtime and engines it has been told to trust |
| **The network** | Observe or answer connections the app makes | The app makes none. No file or metadata leaves the computer |

Out of scope: someone who already controls the user's account or the operating system, physical access to an unlocked computer, and recovery of deleted data from the disk (deletion is not secure erasure).

## Trust boundaries

```
renderer (web view)  ->  core (Rust)  ->  worker (Python)  ->  engine (FFmpeg, LibreOffice, ...)
   untrusted UI           trusted         untrusted            untrusted
                                          \_______ all inside the per-job sandbox _______/
```

- **Renderer to core.** The interface shows only the app's own pages. It has no file, shell, network, dialog or clipboard permission; it calls the core's commands by name, and the core opens native dialogs and handles files. The web view's content security policy allows no remote sources and navigation away is blocked.
- **Core to worker.** The core never parses a user's file. It sniffs the first and last 64 KB (a fuzzed, memory-safe Rust parser), then starts a worker. Messages cross a typed protocol with size limits; worker output is bounded, and results are re-sniffed before they can be saved.
- **Worker to engine.** Workers run engines as child processes, and the sandbox grants an adapter only the engines it lists. Engines start with a minimal environment (the job's folder as `HOME` and `TMPDIR`, system folders on `PATH`, and a few Windows variables), not the worker's.
- **The sandbox** wraps every worker and everything it starts. It can read the runtime and tools, and read and write the job's own folder; nothing else, and no network. If the self-test fails, processing is off.

## Mitigations

| Threat | Mitigation | Where |
|---|---|---|
| Parser exploits in untrusted files | Per-job sandbox, one profile per job | `crates/sandbox/` (`macos.rs`, `linux.rs`, `windows.rs`) |
| Sandbox silently not working | Self-test with probes at every start; processing off if it fails | `crates/sandbox/src/probe.rs`, `crates/app-core/src/lib.rs` |
| Malformed or disguised files | Sniffer by content, not extension; fuzzed nightly | `crates/sniffer/`, `crates/sniffer/fuzz/`, `.github/workflows/nightly.yml` |
| Runaway workers, memory, time, output size | Resource budgets, watchdog, limits on pages, files, pixels and frames | `crates/scheduler/`, `workers/sdk/src/converter_sdk/limits.py` |
| Network access | Sandbox denies the network; no network code; canary test and packet capture | `crates/app-core/tests/network.rs`, `tools/network-test/capture.sh` |
| Active content in documents (macros, scripts, links) | Never followed; "make safe to open" removes it and an independent verifier checks | `adapters/file-disarm/`, `workers/sdk/src/converter_sdk/active.py` |
| Hidden details in results | "Remove hidden details" re-reads each result and fails if any remains; redaction verifies its output | `adapters/file-clean/`, `adapters/media-clean/`, `adapters/pdf-redact/` |
| Compromised renderer | No privileged permissions; fixed content security policy; navigation blocked | `apps/desktop/src-tauri/capabilities/`, `tauri.conf.json`, `src/navigation.rs` |
| Writes to the wrong place, symlinks and odd names | Saving only to the chosen folder; names sanitised; outputs must be regular files | `crates/jobstore/src/save.rs`, `crates/jobstore/src/name.rs`, `fsutil.rs` |
| Data left behind | Job folders deleted on save, discard and quit; stale ones removed at start; refuses a data folder inside a sync folder | `crates/jobstore/` (`store.rs`, `sync.rs`, `data_root.rs`) |
| Passwords leaking | Held in memory only, kept out of logs and command lines | `crates/protocol/src/secret.rs` |
| Hostile runtime or engine on the search path | The runtime is the app's own; development mode is opt-in (`LFCS_DEV=1`), must be inside the repository and owned by the user; engines are looked up only in an allow-list of install locations | `crates/app-core/src/config.rs`, `crates/app-core/src/engines.rs` |
| Redaction or disarm output that is wrong | The core re-checks it in a separate job ([ADR 0006](adr/0006-independent-output-recheck.md)) | `docs/adr/0006-independent-output-recheck.md` |
| Vulnerable or unlicensed dependencies | `cargo-deny`, `pip-audit`, Dependabot, actions pinned to commit SHAs | `deny.toml`, `.github/` |

This table names where each lives; it does not say each one is verified on every system.

## Known gaps

- **Windows sandbox.** The AppContainer and Job Object sandbox is written, and its tests pass on a hosted Windows runner, but it has not been confirmed on real machines. It is the least tested of the three, and the network test doesn't run on Windows.
- **Windows AppContainer ACEs.** The app grants the container access entries on its own folders. They are not removed when the app is uninstalled; that needs an installer.
- **Linux system-call filter.** Bubblewrap isolates namespaces (it drops all capabilities, blocks nested user namespaces and caps processes). A hand-written seccomp deny-list refuses ptrace, mounts, namespaces, kernel modules, BPF, keyrings and clocks, and the 32-bit and x32 system-call interfaces; everything else is allowed. It is built for x86-64 and AArch64 only, and the sandbox is the namespaces alone on another architecture. Its tests pass on GitHub's Ubuntu 24.04 runner; it hasn't been tried elsewhere.
- **WebView traffic is unmeasured.** The operating system's web view may make its own background connections (certificate checks, updates). Spike S4 will list them. The sandbox does not cover the web view.
- **Engines are not pinned.** FFmpeg and LibreOffice come from the machine; there are no pinned versions or checksums until packaging (see the [release plan](release-plan.md)).
- **No installer.** The app runs from a checkout in development mode, which trusts that checkout's `.venv`. A script builds the bundled runtime a release would use (`tools/bundle-runtime`), and a workflow builds unsigned installers from it; neither has run yet.
- **macOS stray processes.** A worker's background process is found by an environment tag. Apple's own binaries hide their environment, so one of those can't be found; Python workers are. The macOS process cap guards against fork bombs on the user's total, not per job.
- **Deletion is not secure erasure,** and sync-folder detection is best effort.
- **A sandboxed worker can still be tricked into wrong output.** The checks (verifiers, re-sniffing, size limits) reduce this; they don't make a result a guarantee.
