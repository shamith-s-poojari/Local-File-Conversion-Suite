# dev-fake

**Development only. Never shipped** (`devOnly: true` in the manifest).

A stand-in adapter for the walking skeleton (spec section 3, week 3), the conformance kit (T14) and the scheduler's supervisor tests (T12). It copies the first input to `out/` unchanged while reporting simulated per-page progress.

| | |
|---|---|
| Kind | operation |
| Tier | E (the output is a byte copy of the input) |
| Engine | none |
| License | AGPL-3.0-only, like the rest of the repo |

## Fake behavior

It parses nothing. It treats byte patterns as facts, which is fine for fixtures and wrong for real files:

- **Corrupt:** an input without a `%PDF-` header gives `E_INPUT_CORRUPT`.
- **Encrypted:** an input containing `/Encrypt` needs the password `fake-password`. With no password it gives `E_PASSWORD_REQUIRED`; with any other password, `E_PASSWORD_WRONG`.
- **Page count:** `inspect` counts `/Type /Page` occurrences.
- **Preview:** `render` writes a blank US Letter PNG at the requested DPI.
- **Page text:** `pageText` returns one word, `fake`.

## Test parameters

See [`params.schema.json`](params.schema.json).

| Parameter | Effect |
|---|---|
| `pages`, `delayMs` | Length of the simulated work |
| `warn` | Emits the given warning code once |
| `failWith` | Fails with a catalog error after page 1 |
| `crash` | Raises an unexpected exception after page 1, giving `E_INTERNAL` |
| `hang` | Ignores `cancel` and never finishes. Only the watchdog, or closing stdin, stops it |
| `noise` | Calls `print()` during execute, to prove stray stdout can't corrupt the protocol |

## Running it by hand

From a job directory containing `in/`, `out/` and `work/`:

```bash
uv run python -I path/to/adapters/dev-fake/adapter.py j_manual
```

Then type NDJSON requests on stdin, for example:

```json
{"v":1,"type":"execute","jobId":"j_manual","inputs":["in/0.pdf"],"outDir":"out","workDir":"work","params":{"pages":3}}
```
