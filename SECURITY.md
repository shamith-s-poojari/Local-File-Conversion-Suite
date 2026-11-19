# Security Policy

## Supported Versions

| Version | Status |
|---|---|
| `0.1.0-dev` (current) | Pre-release — actively developed, security reports accepted |

This project is in active pre-release development. Security reports are taken seriously at every stage.

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Please report security issues by emailing:

**security@shamith-s-poojari.dev**

> *Maintainer: replace this address with your actual security contact before publishing.*

### What to Include

- **Description** of the vulnerability
- **Affected component** (sandbox, sniffer, protocol, specific adapter, etc.)
- **Steps to reproduce** the issue
- **Impact assessment** — what an attacker could achieve
- **Proof of concept** if available (file samples, commands, logs)
- **Suggested fix** if you have one

### Response Timeline

| Stage | Timeline |
|---|---|
| **Acknowledgment** | Within 48 hours |
| **Triage and assessment** | Within 7 days |
| **Fix development** | Depends on severity — critical issues are prioritized immediately |
| **Disclosure** | Coordinated with the reporter |

## Security Model

Local File Conversion Suite is designed with a defense-in-depth security model:

### No Network — By Design

The application contains **zero network code**. There are no HTTP clients, WebSocket connections, telemetry endpoints, crash reporters, update checkers, or font downloaders. There is no code path that can transmit data off the machine.

This is verified by:
- **Canary tests** (`crates/app-core/tests/network.rs`): every tool processes documents whose metadata contains URLs pointing at a local listener. The test fails if anything connects.
- **Packet capture tests** (`tools/network-test/capture.sh`): records every packet during a full processing run and fails on any packet from the app's processes.

### Per-Job OS Sandbox

Every processing job runs in its own operating system sandbox:

| Platform | Technology | Status |
|---|---|---|
| macOS | Seatbelt (sandbox-exec) | Working |
| Linux | Bubblewrap (bwrap) | Written, not yet verified |
| Windows | AppContainer + Job Object | Written, compile-checked only |

Each sandbox can **only** access:
- The application's own runtime and tools (read-only)
- The job's private working directory (read-write)

It **cannot** access:
- The user's home directory or any personal files
- Other jobs' directories
- The network (all protocols blocked)
- Other running processes

### Sandbox Self-Test

At every application start, the sandbox runs a self-test. **If the self-test fails, all processing is disabled.** The app never runs a conversion engine without sandbox protection.

### Data Isolation

- File bytes **never** cross the UI IPC boundary — the WebView only receives events, paths, and metadata
- Passwords are held in **memory only** — never written to disk, logs, or command-line arguments
- **No history** is kept — no recent files list, no document text in logs, nothing survives a restart
- Job directories are deleted when the user saves or discards a result, and all remaining directories are cleaned on quit and on next start (crash recovery)

## Scope — What Counts as a Vulnerability

The following are considered security vulnerabilities:

- **Sandbox escape** — a crafted file that causes a worker to access files or resources outside its permitted sandbox
- **Data exfiltration** — any mechanism that causes file content, metadata, or passwords to leave the user's machine
- **Path traversal** — a file name, parameter, or adapter input that causes reading or writing outside the job directory
- **Code execution outside sandbox** — a crafted input that executes code in the trusted Rust core or the UI process
- **Privilege escalation** — an adapter or worker gaining capabilities beyond what the sandbox profile permits
- **Persistent data leak** — file content, passwords, or personal data persisting on disk after job cleanup
- **Protocol injection** — crafted NDJSON messages that cause the core to perform unintended actions
- **Cross-job access** — one job's worker reading or modifying another job's files

## Out of Scope

The following are **not** considered vulnerabilities for this project:

- **Denial of service against the local app** — resource exhaustion or crashes from large or malformed files (these are bugs, not security issues — please report them as regular issues)
- **Physical access attacks** — an attacker with physical or root access to the machine
- **Social engineering** — tricking a user into processing a file they shouldn't
- **Secure erasure** — deleted files may be recoverable with forensic tools (documented in the [Privacy Policy](docs/privacy.md); users needing this level of protection should use full-disk encryption)
- **Font substitution in Office conversions** — layout differences due to missing fonts
- **WebView OS-level connections** — the system WebView component may make its own background connections (e.g., certificate revocation checks); these are documented as a known gap pending investigation

## Recognition

We appreciate the security research community. Reporters of valid vulnerabilities will be credited in the project's release notes and in a Hall of Fame section (unless they prefer to remain anonymous).

## Disclosure Policy

This project follows **coordinated disclosure**:

1. The reporter privately shares the vulnerability details
2. The maintainers acknowledge, assess, and develop a fix
3. A fix is released, and the vulnerability is disclosed publicly
4. The reporter is credited (unless they opt out)

We ask reporters to allow a reasonable timeframe for fixes before public disclosure — typically 90 days, though we aim to resolve critical issues much faster.

---

Thank you for helping keep Local File Conversion Suite secure.
