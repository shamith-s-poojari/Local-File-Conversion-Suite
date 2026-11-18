"""Sandbox probe (spec section 9.3). Not an adapter: it never ships in a registry.

Run inside the sandbox as ``python -I probe.py access CONFIG_JSON`` or
``python -I probe.py alloc`` or ``python -I probe.py engine CONFIG_JSON`` (run
with an engine grant: its folders must be readable and not writable). In
``access`` mode it tries everything the sandbox must refuse, plus two things it must allow (so a probe that can't do
anything doesn't pass by accident), and prints a JSON report. In ``alloc``
mode it allocates until something stops it.
"""

import errno
import json
import os
import socket
import subprocess
import sys
import time


def codes(*names):
    """errno values by name; names this OS doesn't have are skipped."""
    return {getattr(errno, n) for n in names if hasattr(errno, n)}


# A refusal is only evidence if it is the refusal the sandbox gives. Any other
# failure (a typo'd path, a full disk, a crash) says nothing about the
# sandbox and is reported as "inconclusive", which fails the self-test.
#
# macOS (Seatbelt) answers EPERM; Windows (AppContainer) answers access
# denied (EACCES) or a Winsock WSAEACCES; Linux (bubblewrap) leaves outside
# paths absent (ENOENT) and the network empty (ENETUNREACH, or ECONNREFUSED
# for loopback: bwrap brings its own loopback up, with nothing listening).
DENIED = codes("EPERM", "EACCES", "WSAEACCES")
ABSENT = codes("ENOENT", "ENOTDIR") if sys.platform.startswith("linux") else set()
NETWORK = DENIED | codes("ENETUNREACH", "WSAENETUNREACH", "EHOSTUNREACH")
LOOPBACK = NETWORK | (codes("ECONNREFUSED") if sys.platform.startswith("linux") else set())
# A connect that is dropped rather than refused (Windows loopback isolation).
TIMED_OUT = sys.platform == "win32"
RESOLVER = {
    getattr(socket, n)
    for n in ("EAI_NONAME", "EAI_AGAIN", "EAI_FAIL", "EAI_SERVICE")
    if hasattr(socket, n)
}


# Windows error codes for a process the sandbox won't start: access denied,
# or the Job Object's process cap (ERROR_NOT_ENOUGH_QUOTA). Python maps the
# second to no specific errno, so it is matched by its Windows code.
SPAWN_REFUSED_WIN = {5, 1816}


def attempt(fn, expected, winerrors=frozenset()):
    try:
        fn()
    except OSError as e:
        code = e.errno
        if (
            code in expected
            or getattr(e, "winerror", None) in winerrors
            or (TIMED_OUT and isinstance(e, TimeoutError))
        ):
            return {"outcome": "blocked", "errno": code}
        return {"outcome": "inconclusive", "error": type(e).__name__, "errno": code}
    except Exception as e:  # noqa: BLE001 - not a refusal; record what it was
        return {"outcome": "inconclusive", "error": type(e).__name__}
    return {"outcome": "allowed"}


def connect(host, port):
    s = socket.create_connection((host, port), timeout=3)
    s.close()


def connect_unix(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.settimeout(3)
        s.connect(path)
    finally:
        s.close()


def shell():
    if sys.platform == "win32":
        root = os.environ.get("SYSTEMROOT", r"C:\Windows")
        return [os.path.join(root, "System32", "cmd.exe"), "/c", "exit 0"]
    # /usr/bin/sh exists inside the Linux sandbox (it binds /usr), so a
    # missing /bin/sh there would pass for the wrong reason.
    return ["/usr/bin/sh" if os.path.exists("/usr/bin/sh") else "/bin/sh", "-c", "true"]


def proc_environ(expected):
    """/proc/1/environ: refused, or readable but holding nothing but the
    scrubbed worker variables (pid 1 is the sandbox's own init, not the host's)."""
    try:
        with open("/proc/1/environ", "rb") as f:
            data = f.read()
    except OSError as e:
        if e.errno in expected:
            return {"outcome": "blocked", "errno": e.errno}
        return {"outcome": "inconclusive", "error": type(e).__name__, "errno": e.errno}
    names = {v.split(b"=", 1)[0].decode(errors="replace") for v in data.split(b"\0") if v}
    return {"outcome": "scrubbed" if names <= set(os.environ) else "allowed"}


def list_home(cfg, expected):
    """Refused; or (Linux) listable but only as the skeleton bubblewrap makes
    above its mounts: none of the real names the core sent is there."""
    result = attempt(lambda: os.listdir(cfg["home"]), expected)
    if result["outcome"] != "allowed" or not sys.platform.startswith("linux"):
        return result
    real = cfg.get("homeEntries") or []
    if not real:
        return {"outcome": "inconclusive", "error": "noHomeEntries"}
    seen = set(os.listdir(cfg["home"]))
    return {"outcome": "allowed" if seen & set(real) else "skeleton"}


def ptrace_refused():
    """Linux: the seccomp filter answers ptrace with EPERM. The target pid
    doesn't exist, so an unfiltered kernel says ESRCH and nothing is traced."""
    import ctypes

    libc = ctypes.CDLL(None, use_errno=True)
    libc.ptrace.restype = ctypes.c_long
    libc.ptrace.argtypes = [ctypes.c_long, ctypes.c_long, ctypes.c_void_p, ctypes.c_void_p]
    PTRACE_PEEKDATA = 2
    ctypes.set_errno(0)
    libc.ptrace(PTRACE_PEEKDATA, 0x3FFFFFFF, None, None)
    code = ctypes.get_errno()
    if code == errno.EPERM:
        return {"outcome": "blocked", "errno": code}
    if code == errno.ESRCH:
        return {"outcome": "allowed", "errno": code}
    return {"outcome": "inconclusive", "errno": code}


def engine(cfg):
    """Each folder an engine is granted must be readable and not writable."""
    report = {}
    for i, d in enumerate(cfg["dirs"]):
        report[f"engine_read_{i}"] = attempt(lambda d=d: os.listdir(d), set())
        probe_file = os.path.join(d, ".lfcs-probe")

        def write(path=probe_file):
            with open(path, "w") as f:
                f.write("probe")

        # Read-only mounts answer EROFS, Seatbelt and AppContainers EPERM/EACCES.
        result = attempt(write, DENIED | codes("EROFS"))
        if result["outcome"] == "allowed":
            try:  # leave the engine folder as found
                os.remove(probe_file)
            except OSError:
                pass
        report[f"engine_write_{i}"] = result
    sys.stdout.write(json.dumps(report))


def access(cfg):
    def read(path):
        with open(path, "rb") as f:
            f.read(16)

    def write(path):
        with open(path, "w") as f:
            f.write("probe")

    def through_symlink():
        os.symlink(cfg["sentinel"], "work/escape")
        read("work/escape")

    def hard_link():
        os.link(cfg["sentinel"], "work/linked")

    fs = DENIED | ABSENT
    report = {
        "read_job": attempt(lambda: read("in/probe.txt"), set()),
        "write_work": attempt(lambda: write("work/probe.out"), set()),
        "read_sentinel": attempt(lambda: read(cfg["sentinel"]), fs),
        "list_home": list_home(cfg, fs),
        "read_other_job": attempt(lambda: read(cfg["otherJobFile"]), fs),
        "write_outside": attempt(lambda: write(cfg["outsideFile"]), fs | codes("EROFS")),
        "connect_external": attempt(lambda: connect("1.1.1.1", 443), NETWORK),
        "connect_loopback": attempt(lambda: connect("127.0.0.1", cfg["loopbackPort"]), LOOPBACK),
        "dns": attempt(
            lambda: socket.getaddrinfo("example.com", 80),
            NETWORK | RESOLVER,
        ),
        "spawn_shell": attempt(
            lambda: subprocess.run(shell(), check=True),  # noqa: S603
            fs,
            SPAWN_REFUSED_WIN,
        ),
        # A hard link makes the outside file part of the job directory, and
        # every later read of it allowed.
        "hardlink_out": attempt(hard_link, fs | codes("EXDEV")),
    }
    if sys.platform != "win32":  # creating a symlink needs a privilege there
        report["symlink_out"] = attempt(through_symlink, fs)
        report["unix_socket"] = attempt(lambda: connect_unix(cfg["socket"]), fs)
    if sys.platform.startswith("linux"):
        report["proc_environ"] = proc_environ(fs)
        report["seccomp_ptrace"] = ptrace_refused()
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
    elif sys.argv[1] == "engine":
        engine(json.loads(sys.argv[2]))
    else:
        alloc()
