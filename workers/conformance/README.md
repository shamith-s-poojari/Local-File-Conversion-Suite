# Conformance kit

Runs the checks in [spec section 8.4](../../docs/specs/phase-1.md#84-conformance-kit-workersconformance) against every adapter in `adapters/` that isn't `devOnly`. `uv run pytest` runs it with everything else.

Each adapter provides `tests/conformance.yaml`:

```yaml
cases:                 # at least one; the first is also used for statelessness
  - name: basic
    inputs: [text-3, text-5]       # names from converter_conformance.corpus.BUILDERS
    params: { bookmarks: perFile }
    secrets: { userPassword: x }   # optional
    expect: { pages: [8], password: x }   # page count per output; password to read them
negative:              # params (and optional secrets) used with corrupt, truncated and encrypted inputs
  params: {}
cancel:                # a job long enough to cancel mid-run
  inputs: [big]
  params: {}
  repeat: { param: pages, item: { source: 1 }, times: 4000 }   # optional: build a long list param
```

| Check (spec 8.4) | How |
|---|---|
| Corrupt and truncated input | Structured error, never `E_INTERNAL` or `E_PROTOCOL`. A truncated file that the engine repairs may succeed instead, if the output is valid |
| Encrypted input, no password | `E_PASSWORD_REQUIRED` |
| Output containment | Nothing new outside `out/` and `work/` in the job directory |
| Cancellation | Error `E_CANCELLED` within 2 s of `cancel` |
| Memory estimate | Peak RSS ≤ 1.5 × `ramBytes` (macOS and Linux; Windows has no `wait4`) |
| Output validity | Opens, `check()` finds no problems, page counts match `expect` |
| Warnings | Every warning is in the catalog and in the manifest's `fidelityNotes` |
| Secret handling | The sentinel password isn't in protocol output, stderr or any file in the job directory |
| Statelessness | Two runs give the same pages, compared by decoded content |
| Timeout | Not here: the supervisor's watchdog is tested with `dev-fake` in `crates/scheduler/tests/runner.rs` |

Workers run **without the sandbox** here. Sandbox enforcement is checked by the probe (spec section 9.3).

`converter_conformance.corpus` generates every test PDF, so the kit needs no binary fixtures.
