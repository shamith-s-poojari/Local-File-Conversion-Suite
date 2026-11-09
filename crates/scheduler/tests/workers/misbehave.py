"""A hand-written worker that breaks the protocol on purpose, for supervisor tests.

Usage: misbehave.py <jobId> <adapterId> <mode>

It answers validate and estimate normally (unless the mode says otherwise)
and misbehaves in execute. It needs nothing but the standard library.
"""

import hashlib
import json
import os
import subprocess
import sys
import time

job_id, adapter_id, mode = sys.argv[1], sys.argv[2], sys.argv[3]
out = sys.stdout.buffer

PDF = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n"
PNG = b"\x89PNG\r\n\x1a\n" + b"\x00" * 64


def send(msg, job=None):
    msg = {"v": 1, "jobId": job or job_id, **msg}
    out.write(json.dumps(msg).encode() + b"\n")
    out.flush()


def write_output(name, data):
    os.makedirs("out", exist_ok=True)
    with open(os.path.join("out", name), "wb") as f:
        f.write(data)
    return {
        "path": f"out/{name}",
        "suggestedName": name,
        "sizeBytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    }


if mode == "no_hello":
    time.sleep(60)
    sys.exit(0)

version = 99 if mode == "bad_version" else 1
send({"type": "hello", "adapterId": adapter_id, "protocolVersion": version, "pid": os.getpid()})

for raw in sys.stdin.buffer:
    req = json.loads(raw)
    kind = req["type"]
    if kind == "cancel":
        send({"type": "error", "code": "E_CANCELLED", "retryable": False})
        continue
    if kind == "validate":
        if mode == "wrong_job":
            result = {"type": "validateResult", "errors": [], "warnings": [], "metadata": {}}
            send(result, "j_other")
        elif mode == "garbage":
            out.write(b"this is not json\n")
            out.flush()
        elif mode == "long_line":
            out.write(b"x" * (2 * 1024 * 1024) + b"\n")
            out.flush()
        elif mode == "flood":
            for i in range(1500):
                send({"type": "progress", "stage": "check", "done": i})
        elif mode == "wrong_result":
            send({"type": "estimateResult", "ramBytes": 1, "diskBytes": 1, "costUnits": 1})
        else:
            send({"type": "validateResult", "errors": [], "warnings": [], "metadata": {}})
        continue
    if kind == "estimate":
        send({"type": "estimateResult", "ramBytes": 1000, "diskBytes": 1000, "costUnits": 1})
        continue
    if kind != "execute":
        continue

    if mode == "bad_hash":
        o = write_output("o.pdf", PDF)
        o["sha256"] = "0" * 64
        send({"type": "executeResult", "outputs": [o], "fidelity": "E"})
    elif mode == "bad_size":
        o = write_output("o.pdf", PDF)
        o["sizeBytes"] += 1
        send({"type": "executeResult", "outputs": [o], "fidelity": "E"})
    elif mode == "wrong_format":
        send({"type": "executeResult", "outputs": [write_output("o.pdf", PNG)], "fidelity": "E"})
    elif mode == "unknown_warning":
        send({"type": "warning", "code": "MADE_UP_CODE", "detail": {"note": "x" * 5000}})
        send({"type": "executeResult", "outputs": [write_output("o.pdf", PDF)], "fidelity": "E"})
    elif mode == "memory_once":
        if os.path.exists("memory-marker"):
            outputs = [write_output("o.pdf", PDF)]
            send({"type": "executeResult", "outputs": outputs, "fidelity": "E"})
        else:
            open("memory-marker", "w").close()
            send({"type": "error", "code": "E_RESOURCE_MEMORY", "retryable": True})
    elif mode == "memory_always":
        send({"type": "error", "code": "E_RESOURCE_MEMORY", "retryable": True})
    elif mode == "grandchild_hang":
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
        with open("child.pid", "w") as f:
            f.write(str(child.pid))
        while True:
            time.sleep(1)
    elif mode == "exit_silently":
        sys.exit(3)
    else:
        send({"type": "executeResult", "outputs": [write_output("o.pdf", PDF)], "fidelity": "E"})
