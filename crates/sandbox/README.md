# converter-sandbox

Per-job OS sandbox for workers, the runner that enforces memory limits, and the self-test probe ([spec section 9](../../docs/specs/phase-1.md#9-sandbox-cratessandbox)).

## How a worker starts

```text
converter-sandbox-run --memory-limit N --job-id J -- <OS sandbox> python -I adapters/<id>/adapter.py J
```

| Layer | Job |
|---|---|
| `SandboxLauncher` | Implements the scheduler's `Launcher`. Validates the adapter ID, scrubs and rebuilds the environment (`PATH`, `LANG`, `TMPDIR` and `HOME` set to `work/`, `OMP_THREAD_LIMIT=1`) and builds the command. The working directory is the job directory |
| `converter-sandbox-run` | Outside the OS sandbox. Starts the worker with inherited stdio, sums the resident memory of the worker and its descendants every 50 ms, and on a breach kills them and writes a protocol `error` with `E_RESOURCE_MEMORY`. Exit status mirrors the worker's. It stays in the scheduler's process group, so a group kill reaches everything |
| OS sandbox | **macOS:** a per-job Seatbelt profile through `sandbox-exec`. **Linux:** bubblewrap. **Windows:** not yet (T09); the launcher refuses, so jobs fail with `E_SANDBOX_UNAVAILABLE`, never unsandboxed |

`SandboxConfig::discover` asks the interpreter where its runtime lives: prefixes, site-packages, every `sys.path` entry (editable installs live outside site-packages), and the install root it may execute from.

## What a worker can do

| | macOS | Linux |
|---|---|---|
| Read | System libraries, the interpreter and its packages, `adapters/`, the job directory. Metadata (names, sizes) anywhere; listing `/` | Read-only binds of `/usr`, `/lib*`, the runtime and `adapters/`; the job directory |
| Write | The job directory, `/dev/null` | The job directory, a private `/tmp` |
| Execute | Binaries inside the interpreter's install only | Everything bound into the namespace (read-only) |
| Network | None: sockets, loopback and DNS fail | Empty network namespace |
| Memory | RSS watchdog in the runner | `RLIMIT_AS` backstop (4 × limit + 2 GiB) plus the RSS watchdog |

Executing the interpreter again is allowed on purpose: `pdf-redact` verifies its output in a fresh process. `/bin/sh` and every other binary are denied on macOS.

## Self-test

`probe::self_test` runs [`probe/probe.py`](probe/probe.py) (embedded in the binary) in a scratch job. It must be **blocked** from:

- reading a sentinel outside the job, listing the home directory, reading another job's file, writing outside the job;
- connecting out or to a loopback listener, resolving DNS;
- spawning a shell.

It must be **allowed** to read `in/` and write `work/`, so a probe that can't do anything doesn't pass by accident. A second run allocates past 256 MB and must end with `E_RESOURCE_MEMORY` from the runner. If the self-test fails, the app shows an error and disables processing (spec 9.3).

`tests/sandbox.rs` runs the probe and a real job through the scheduler with this launcher. On macOS both must pass. On Linux they skip when bubblewrap can't create namespaces, for example on Ubuntu 24.04 without the AppArmor profile (C7).

## Known gaps

- **Windows (T09)** isn't implemented.
- **Linux** is written to the spec but not yet run on a Linux machine; CI is its first real test.
- The worker's peak memory isn't reported back to the core yet (spec 9.1 `peak_memory`).
- If the runner kills a worker while it is mid-way through writing a protocol line, the scheduler sees a malformed line (`E_PROTOCOL`) rather than `E_RESOURCE_MEMORY`. Workers write each line in one call, so this needs a breach at that exact moment.
