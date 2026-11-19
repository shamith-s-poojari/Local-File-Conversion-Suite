# Network test

Proves that processing makes no network connections ([spec 14.5](../../docs/specs/phase-1.md#145-network-test-c4), change C4). Two layers:

| Layer | Where | Needs | What it proves |
|---|---|---|---|
| Canary and socket sampler | `crates/app-core/tests/network.rs`, part of `cargo test` (macOS and Linux; the sampler reads `lsof` and the process tree, so on Windows the test prints "skipped" and passes) | Nothing special | No worker, runner or verifier holds an internet socket, and nothing contacts the canary, across every tool and read path |
| Packet capture | `capture.sh`, nightly | Root | No packet leaves the machine from our processes. With `--app`, the idle app's WebView traffic matches the allowlist |

## The canary corpus

[`canary_corpus.py`](canary_corpus.py) builds PDFs whose outward references all point at a local listener:

- link and open actions, page-open and close actions, and form submission
- a remote go-to action
- document JavaScript
- an HTML attachment
- XMP metadata
- an external stream reference

The test runs every operation on them:
- inspect, previews and page text
- merge, split, organize, protect, watermark and redact
- previews of the results

The listener must receive nothing (item 6).

## Positive controls (item 5)

A harness that sees nothing must fail rather than pass:

- **A:** the listener counts a deliberate connection from the test.
- **B:** the sampler catches a child process that holds a connection.
- **C:** the sampler saw at least one worker process during the run.

## Running the capture

```bash
sudo tools/network-test/capture.sh
```

```bash
sudo tools/network-test/capture.sh --app "target/debug/bundle/macos/Local File Conversion Suite.app"
```

| OS | Attribution |
|---|---|
| macOS | `tcpdump -i pktap -k`: the kernel tags each packet with its process |
| Linux | `strace -f` on the test's process tree records every `connect` and `sendto`, with tcpdump as a record |
| Windows | Not wired up (`pktmon`, needs a Windows machine) |

`webview-allowlist.txt` is empty until spike S4 measures and discloses WebView runtime traffic. Until then any WebView destination fails `--app`.

**Status:** `network.rs` passes on macOS and Linux (25 to 28 worker processes watched, no sockets, no canary hits). `capture.sh` runs in the nightly job: it has passed on macOS. On Linux it failed on 2026-11-15 to 17 (`strace` couldn't write its trace into the root-owned temp folder); the script now hands the folder to the test user and fails if the trace holds no `connect()` call at all, and has not run since. Nothing runs on Windows.
