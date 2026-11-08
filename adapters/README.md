# Adapters

One folder per adapter. Adding an adapter changes **no shared file** (spec exit criterion 14): the registry is generated from these folders at build time.

```
adapters/<id>/
├── manifest.yaml        # Checked against manifest.schema.json
├── params.schema.json   # The UI form is generated from this
├── adapter.py
├── tests/
└── README.md            # What it does, tier, known limits, engine, license
```

## Manifest rules

[`manifest.schema.json`](manifest.schema.json) defines the fields (spec section 8.2). `pnpm registry` also fails on:

- an `id` that differs from the folder name, or is used twice
- a `fidelityNotes` code that isn't in the warning catalog (spec section 13.1, `WarningCode` in the protocol schema)
- an engine in `requires` that isn't approved in [`engines/engines.yaml`](../engines/engines.yaml)
- an invalid `params.schema.json`, or an `x-ui` widget not in spec section 12.3
- a conversion edge from a format to itself, or the same edge declared twice

An **operation** (merge, split, redact…) declares `inputs`, `outputs` and `fidelity`. A **conversion** declares `edges` in the conversion graph instead (change C5). Conversions are offered only where the [compatibility matrix](../docs/architecture/blueprint.md#32-matrix) lists the pair.

Set `devOnly: true` for test and development adapters. They're left out of the registry unless the generator runs with `--include-dev`, so they never ship.

## Commands

```bash
pnpm registry
```

Writes `registry.generated.json` (gitignored). `pnpm check` validates every manifest, including dev-only ones.
