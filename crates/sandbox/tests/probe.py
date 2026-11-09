"""Sandbox probe used by tests/windows.rs (a first cut of the spec 9.3 probe).

Usage: probe.py <mode> <json-args>. Prints one JSON object of results to
stdout. Every attempt reports "allowed" or "denied" so the test can tell a
probe that fails for the wrong reason (the allowed controls) from a sandbox
that works.
"""

import json
import os
import socket
import subprocess
import sys
import time


def attempt(fn):
    try:
        fn()
        return "allowed"
    except Exception as e:  # noqa: BLE001
        return "denied:" + type(e).__name__


def read(path):
    with open(path, "rb") as f:
        f.read(1)


def write(path):
    with open(path, "wb") as f:
        f.write(b"x")


def connect(port):
    s = socket.socket()
    s.settimeout(2)
    try:
        s.connect(("127.0.0.1", port))
    finally:
        s.close()


def main():
    mode = sys.argv[1]
    args = json.loads(sys.argv[2])
    if mode == "access":
        out = {
            "read_own": attempt(lambda: read(args["own_file"])),
            "write_work": attempt(lambda: write(os.path.join(args["work"], "probe.txt"))),
            "read_outside": attempt(lambda: read(args["outside_file"])),
            "list_outside": attempt(lambda: os.listdir(args["outside_dir"])),
            "read_other_job": attempt(lambda: read(args["other_job_file"])),
            "write_outside": attempt(lambda: write(os.path.join(args["outside_dir"], "w.txt"))),
            "loopback": attempt(lambda: connect(args["port"])),
            "dns": attempt(lambda: socket.getaddrinfo("example.com", 443)),
            "spawn": attempt(
                lambda: subprocess.run([sys.executable, "-c", "pass"], check=True, timeout=10)
            ),
            "env": sorted(os.environ),
            "temp": os.environ.get("TEMP"),
            "localappdata": os.environ.get("LOCALAPPDATA"),
        }
    elif mode == "memory":
        def grab():
            blocks = []
            for _ in range(args["mb"]):
                blocks.append(bytearray(1024 * 1024))
        out = {"allocate": attempt(grab)}
    elif mode == "sleep":
        with open(os.path.join(args["work"], "pid"), "w") as f:
            f.write(str(os.getpid()))
        time.sleep(60)
        out = {}
    else:
        raise SystemExit(f"unknown mode {mode}")
    print(json.dumps(out), flush=True)


main()
