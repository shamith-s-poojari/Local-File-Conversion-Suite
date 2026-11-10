"""Sandbox probe (spec section 9.3). Not an adapter: it never ships in a registry.

Run inside the sandbox as ``python -I probe.py access CONFIG_JSON`` or
``python -I probe.py alloc``. In ``access`` mode it tries everything the
sandbox must refuse, plus two things it must allow (so a probe that can't do
anything doesn't pass by accident), and prints a JSON report. In ``alloc``
mode it allocates until something stops it.
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
    except Exception as e:  # noqa: BLE001 - any refusal counts; record its class
        return {"outcome": "blocked", "error": type(e).__name__}
    return {"outcome": "allowed"}


def connect(host, port):
    s = socket.create_connection((host, port), timeout=3)
    s.close()


def access(cfg):
    def read(path):
        with open(path, "rb") as f:
            f.read(16)

    def write(path):
        with open(path, "w") as f:
            f.write("probe")

    report = {
        "read_job": attempt(lambda: read("in/probe.txt")),
        "write_work": attempt(lambda: write("work/probe.out")),
        "read_sentinel": attempt(lambda: read(cfg["sentinel"])),
        "list_home": attempt(lambda: os.listdir(cfg["home"])),
        "read_other_job": attempt(lambda: read(cfg["otherJobFile"])),
        "write_outside": attempt(lambda: write(cfg["outsideFile"])),
        "connect_external": attempt(lambda: connect("1.1.1.1", 443)),
        "connect_loopback": attempt(lambda: connect("127.0.0.1", cfg["loopbackPort"])),
        "dns": attempt(lambda: socket.getaddrinfo("example.com", 80)),
        "spawn_shell": attempt(lambda: subprocess.run(["/bin/sh", "-c", "true"], check=True)),
    }
    sys.stdout.write(json.dumps(report))


def alloc():
    blocks = []
    for _ in range(256):  # up to 16 GiB in 64 MiB steps
        block = bytearray(64 * 1024 * 1024)
        for i in range(0, len(block), 4096):
            block[i] = 1  # touch every page so it's resident
        blocks.append(block)
        time.sleep(0.02)
    sys.stdout.write("survived\n")


if __name__ == "__main__":
    if sys.argv[1] == "access":
        access(json.loads(sys.argv[2]))
    else:
        alloc()
