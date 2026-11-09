# converter-scheduler

Admission scheduler, worker supervisor and watchdog ([spec sections 7.4, 10.2 and 10.3](../../docs/specs/phase-1.md#10-scheduler-job-store-and-garbage-collection)).

## Job flow

`Runner::start` takes a staged job and runs it on its own thread:

1. `validating`: launch a worker, wait for `hello`, send `validate`. Any validation error fails the job with that code.
2. Send `estimate`, then stop the worker so nothing idles while queued.
3. `queued`: wait for admission.
4. `running`: launch a fresh worker with the admitted memory limit and send `execute`.
5. `verifying`: after the worker exits, re-check every output.
6. `succeeded`, or `failed` with a catalog code, or `cancelled`.

## Rules

| Area | Behavior |
|---|---|
| Budget | `min(60% of available RAM, user setting)`, re-read before each admission round. Running jobs' reservations are added back to "available" so they aren't counted twice |
| Hard limit | `min(max(1.5 × estimate, 512 MB), budget)`, passed to the launcher (the sandbox enforces it) |
| Admission | First job in the queue that fits its pool and the budget. A skipped job that has waited 60 s blocks every job behind it |
| Large jobs | Estimate above the whole budget: runs alone with the whole budget, and gets `W_LARGE_JOB` |
| Memory retry | `E_RESOURCE_MEMORY` retries once, alone, at the full budget; a second one fails the job |
| Pools | `pdf` 2, `render` 2, `redact` 1 (`pdf-redact` uses `redact`); others 1 |
| Handshake | `hello` within 10 s with the right job, adapter and protocol version. Version or adapter mismatch: `E_INSTALL_CORRUPT` |
| Messages | Over 1 MiB, invalid JSON, schema failure, wrong `jobId`, wrong result type, or over 1,000 per second: kill the tree, `E_PROTOCOL` |
| Watchdog | `timeoutBaseS + timeoutSPerMb × input MB` per request, and `noProgressS` of silence. Sends `cancel`, waits 2 s, kills the tree: `E_TIMEOUT` |
| Cancel | Same sequence, `E_CANCELLED` |
| Outputs | Inside `out/` with no links, claimed size and SHA-256 must match (`E_PROTOCOL`), and must sniff as the declared format (`E_OUTPUT_INVALID`) |
| Warnings | Codes outside the catalog become `W_UNKNOWN` and the code is logged. Details are cut to 1,000 bytes and are plain text |
| Events | Stages, warnings and state changes at once; progress at most every 100 ms |
| stderr | Last 256 KB in memory only, never written to disk or logged |
| Process tree | Unix: own process group, `SIGKILL` to the group. Windows: a Job Object with kill-on-close |

## Not here

- The sandbox. `Launcher` is implemented by T09 and T10; if it can't apply the sandbox the job fails with `E_SANDBOX_UNAVAILABLE`, with no way to run unsandboxed. The tests use an unsandboxed launcher of their own.
- Preview render priority and superseding (T16).
- Tauri commands and the event bridge to the UI.

## Tests

`tests/runner.rs` runs real workers: the `dev-fake` adapter through the Python SDK, and `tests/workers/misbehave.py`, which breaks the protocol in one way per mode. They need the workspace virtualenv (`uv sync`); without it they print a note and pass.
