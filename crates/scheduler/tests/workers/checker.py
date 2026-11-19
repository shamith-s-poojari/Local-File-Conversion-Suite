"""A stand-in `output-check` worker for the re-check tests (ADR 0006).

Usage: checker.py <jobId> <adapterId> <mode>

It answers `execute` only, like the real check job. Modes:

- ``verdict``: the report says ``passed`` unless an input holds ``BAD``, the
  inputs aren't exactly ``in/0.pdf``, or the params aren't the ones the test
  sent. So a pass also proves what the check job was given.
- ``slow``: runs until the runner stops it, after leaving ``started`` in ``work/``.
- ``crash``: exits before answering.
- ``garbage_report``: the report is not JSON.
- ``no_verdict``: a report without ``passed: true``.
- ``failed_ids``: fails, naming check IDs.

It needs nothing but the standard library.
"""

import hashlib
import json
import os
import sys
import time

job_id, adapter_id, mode = sys.argv[1], sys.argv[2], sys.argv[3]
out = sys.stdout.buffer
EXPECTED_PARAMS = {"verifier": "test", "n": 1}


def send(msg):
    out.write(json.dumps({"v": 1, "jobId": job_id, **msg}).encode() + b"\n")
    out.flush()


def write_report(data):
    os.makedirs("out", exist_ok=True)
    with open("out/report.json", "wb") as f:
        f.write(data)
    return {
        "path": "out/report.json",
        "suggestedName": "report.json",
        "sizeBytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    }


send({"type": "hello", "adapterId": adapter_id, "protocolVersion": 1, "pid": os.getpid()})

for raw in sys.stdin.buffer:
    req = json.loads(raw)
    if req["type"] == "cancel":
        send({"type": "error", "code": "E_CANCELLED", "retryable": False})
        continue
    if req["type"] != "execute":
        continue
    if mode == "crash":
        sys.exit(3)
    if mode == "slow":
        os.makedirs("work", exist_ok=True)
        open("work/started", "w").close()
        time.sleep(60)
    names = sorted(os.listdir("in"))
    bad = any(b"BAD" in open(os.path.join("in", n), "rb").read() for n in names)
    ok = names == ["0.pdf"] and req["params"] == EXPECTED_PARAMS and not bad
    if mode == "garbage_report":
        o = write_report(b"not json")
    elif mode == "no_verdict":
        o = write_report(json.dumps({"passed": "yes"}).encode())
    elif mode == "failed_ids":
        o = write_report(json.dumps({"passed": False, "failed": ["V1", "V3"]}).encode())
    else:
        o = write_report(json.dumps({"passed": ok, "failed": [] if ok else ["V0"]}).encode())
    send({"type": "executeResult", "outputs": [o], "fidelity": "E"})
