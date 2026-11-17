# output-check

The core's second opinion on a tool's output (ADR 0006). A tool whose manifest declares `recheck: redaction` (`pdf-redact`) or `recheck: activeContent` (`file-disarm`) can't succeed until this adapter, in a **separate job**, has passed copies of its outputs.

It has `kind: check`: it is never a conversion target, is not in any tool list and has no UI entry. The core starts it; nothing else can.

- Its inputs are copies of the tool's recorded outputs. It never sees the original inputs, the tool's `work/` folder or the tool's worker.
- `verifier` picks `adapters/pdf-redact/verify.py` or `adapters/file-disarm/verify.py`; those modules share no code with their adapters, so one copy serves the worker's own check and this one. It runs in a fresh `python -I` process.
- For `redaction` the core sends the user's boxes (`regions`), `terms`, `fill` and the original page count (`expectedPages`). It has no text from under the boxes, so the captured-text check (V9) stays in the tool's own worker; V1 to V8 run here.
- For `activeContent` nothing is sent beyond the files.
- The output is `report.json`: `passed`, `failed` (check IDs) and per-input checks with counts. Never document text.

A verifier that can't report is a failed check, never a pass.
