# Worker protocol

The NDJSON protocol between the Rust core and sandboxed workers ([spec section 7](../../docs/specs/phase-1.md#7-worker-protocol-packagesprotocol)).

[`schema/protocol.schema.json`](schema/protocol.schema.json) is the single source of truth. Everything else is generated from it or tested against it.

| Language | Code | How it stays in sync |
|---|---|---|
| TypeScript | [`src/generated.ts`](src/generated.ts), re-exported by [`src/index.ts`](src/index.ts) | Written by `pnpm generate`; `pnpm check` fails if it's stale |
| Python | [`converter_sdk.protocol`](../../workers/sdk/src/converter_sdk/protocol/) (`_generated.py` plus a small wrapper) | Same as TypeScript |
| Rust | [`crates/protocol`](../../crates/protocol/) | Generated at compile time by `typify`, so it can't drift |

[`fixtures/cases.json`](fixtures/cases.json) holds valid and invalid messages. The tests in all three languages run against it, and every message type needs at least one valid case.

## Changing the protocol

1. Edit the schema.
2. Run `pnpm generate` and commit the generated files with the schema change.
3. Add fixtures for the new or changed messages.
4. If you added a message type, map it in `crates/protocol/src/codec.rs`, in the exhaustive matches in `crates/protocol/tests/fixtures.rs`, and in the unions in `converter_sdk/protocol/__init__.py`. The tests fail until you do.

There is no compatibility layer: app and workers ship together (spec section 7.5).

## Decisions made while implementing T02

- **Result messages have their own types:** `inspectResult`, `validateResult`, `estimateResult`, `executeResult`, `renderResult` and `pageTextResult`. The spec table lists a single `result` type whose shape depends on the request. A distinct `type` per shape gives every language a plain discriminated union. The spec's example `execute` exchange is otherwise unchanged, and is a fixture.
- **Warning codes are open on the wire, error codes are closed.** A worker may send a warning code that isn't in the catalog; the core shows it as a generic warning (spec section 7.4). An unknown error code fails the schema, which means `E_PROTOCOL`.
- **`hello` accepts any version number,** so a mismatch is reported as `E_INSTALL_CORRUPT` rather than as a schema failure. Every other message must carry `v: 1`.
- **Paths are checked twice.** The schema allows only relative job paths (no `..`, no leading dots, no backslashes or drive letters), and outputs must be under `out/`. The core still resolves and checks every path on disk.
- **The Rust core validates every line against the schema before deserializing it.** The generated Rust types don't enforce constants or numeric ranges by themselves. Decode errors carry only JSON paths and schema locations, never message content, which may contain document text or passwords.
- **Secrets** are a hand-written type in Rust (zeroized on drop, `Debug` prints `[redacted]`) and `SecretStr` in Python (hidden in `repr` and in encoded output).

## Known gaps in the Python models

Two schema rules aren't enforced when a worker parses a core message: `secrets` may be an empty object, and a missing `v` defaults to 1 so adapters can build messages without it. Both only affect messages from the core, which builds them itself; `workers/sdk/tests/test_protocol.py` lists them.
