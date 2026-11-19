# ADR 0006: The core re-checks redaction and disarm output in a separate job

- Status: accepted
- Date: 2026-11-17
- Builds on: [ADR 0005](0005-safety-privacy-and-convenience.md) (file-disarm and its verifier); spec section 13.8 (redaction verifier)
- Audit finding: S10

## Context

`pdf-redact` and `file-disarm` each check their output with an independent verifier: a separate `python -I` process that shares no code with the adapter and sees only the output. But the adapter starts that process and reads its report. A bug or a hostile input that takes over the adapter's worker can skip the verifier or invent a passing report, and the core would accept the output. For these two tools a false pass is the worst outcome the app has: text the user believes is gone, or a macro they believe was removed.

## Decision

The core runs the verifier itself, in a **separate check job**, before the tool's job may succeed.

1. A tool's manifest can declare `recheck: <verifier id>`. `pdf-redact` declares `redaction`, `file-disarm` declares `activeContent`.
2. When such a job has recorded its outputs and its own worker has exited, the scheduler runs a hidden **check job** whose inputs are copies of those outputs, and nothing else, before the tool's job may leave `verifying`. It gets its own folder (`recheck-<id>` inside the tool's job folder, its whole job directory), its own sandbox profile and its own worker process. It isn't a store job, so it never appears in events or snapshots. It never sees the original inputs, the tool's `work/` folder, or the tool's worker.
3. The check job runs the operation `output-check` (an adapter with no UI entry, `kind: check`). It loads the requested verifier from the tool's adapter folder by path (`adapters/pdf-redact/verify.py`, `adapters/file-disarm/verify.py`). Those modules already share no code with their adapters, so one copy serves both the in-worker check and this one.
4. What the check job is told comes from the core, not from the tool's worker: for redaction, the user's boxes, search terms and fill from the job's params, and the page count the core reads with its own inspect of the input; for disarm, nothing beyond the files. The in-worker check also uses the text the adapter captured under each box; the core never has that text, so the re-check runs the other checks only (V1–V9 except the captured-text match, which stays in the in-worker check).
5. Outcome:
   - every check passes → the tool's job succeeds;
   - any check fails, or the check job fails, times out or can't start → the tool's job fails with `E_REDACTION_VERIFICATION_FAILED` (redaction) or `E_OUTPUT_INVALID` (disarm), and its outputs are deleted. (A job failure carries a code only; the adapter's own check still reports `activeContentRemains` when it catches the problem itself.) A check that can't run is a failed check, never a pass.
   - Cancelling the tool's job cancels its check job. The check job is discarded in every case and never appears in the snapshot.
6. Reports carry only check IDs, pages and counts, never document text, as today.

## Consequences

- Redaction and disarm take roughly the verifier's time twice (once in the worker, once in the check job). The check job queues in the same pool as the tool's job and counts against the tool's timeout budget.
- A take-over of the tool's worker can no longer approve its own output: it would also have to escape the sandbox, or break a separate verifier process it doesn't control.
- The check job copies the outputs. Disk use for these two tools grows by their output size.
- `output-check` must stay out of the router's targets and the UI's tool list.

## Implementation

- `recheck:` in the manifest schema and router; only `output-check` may have `kind: check`, and it is never a target, a tool or startable with `create_job`.
- The hook is in the scheduler's driver (`crates/scheduler/src/runner/recheck.rs`), between recording outputs and the final state, so no `succeeded` state is ever visible for an unchecked output. App-core builds the check params (`crates/app-core/src/recheck.rs`).
- Tests: `crates/scheduler/tests/recheck.rs`, and the sandboxed stand-in adapter in `crates/app-core/tests/common/recheck.rs`, which claims success on a leaky PDF and must end `failed`.
