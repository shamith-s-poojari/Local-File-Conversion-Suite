# output-check

The core's second opinion on a tool's output (ADR 0006). A tool whose manifest declares `recheck: redaction` (`pdf-redact`) or `recheck: activeContent` (`file-disarm`) can't succeed until this adapter, in a **separate job**, has passed copies of its outputs.

It has `kind: check`: it is never a conversion target, is not in any tool list and has no UI entry. The core starts it; nothing else can.

- Its inputs are copies of the tool's recorded outputs. It never sees the original inputs, the tool's `work/` folder or the tool's worker.
- `verifier` picks `adapters/pdf-redact/verify.py`, `adapters/file-disarm/verify.py` or `verify_details.py` (next to this file); those modules share no code with their adapters, so one copy serves the worker's own check and this one. It runs in a fresh `python -I` process.
- For `redaction` the core sends the user's boxes (`regions`), `terms`, `fill` and the original page count (`expectedPages`). It has no text from under the boxes, so the captured-text check (V9) stays in the tool's own worker; V1 to V8 run here.
- For `activeContent` nothing is sent beyond the files.
- `hiddenDetails` re-reads a `file-clean` output (pictures, PDF, Word, Excel, PowerPoint) with `converter_sdk.reveal` and fails if who, where, when, device, tags or embedded files remain. `file-clean` declares no `recheck` in its manifest (that needs a router kind and a `verify.py` in its folder); the core asks for this check by the tool's ID (`crates/app-core/src/recheck.rs`). `media-clean` has none: reading its outputs needs FFprobe, which this adapter's worker doesn't have.
- The original's page count (`expectedPages`) comes from the core's own inspect of the input. This job never sees the original, so it can't count it itself.
- The output is `report.json`: `passed`, `failed` (check IDs) and per-input checks with counts. Never document text.

A verifier that can't report is a failed check, never a pass.
