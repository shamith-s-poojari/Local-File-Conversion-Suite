# Spike S1 (macOS part): sandboxing Python workers

| | |
|---|---|
| **Question** | Can Python with pikepdf, PyMuPDF, pypdfium2, ReportLab and Pillow run under a macOS sandbox that allows only its runtime and one job directory? Which approach: App Sandbox or per-job Seatbelt profiles (change C8)? |
| **Result** | **Pass, with per-job Seatbelt profiles.** Implemented in `crates/sandbox` |
| **Machine** | macOS 27.3 (Apple silicon), Homebrew Python 3.13.4, run on 2026-11-10 |
| **Not covered** | LibreOffice (Phase 2 engine), notarized builds, Intel Macs |

## Decision, by the C8 criteria

1. **Can one job read another job's files?** With Seatbelt, no: each worker gets its own profile naming only its own job directory. The probe confirms it. App Sandbox children inherit the parent's access, so they fail this criterion.
2. **Does it work with a notarized, directly distributed app?** `sandbox-exec` is a system binary that needs no entitlement, and the app itself doesn't need App Sandbox for direct distribution. A notarized build is still to be tested (S2).
3. **Cost to maintain.** One profile generator of about 100 lines. `sandbox-exec` is deprecated but present and working; Apple's own services still use Seatbelt profiles. If it is ever removed, the fallback is App Sandbox with one helper per job, which costs more.

## What the profile needed

| Rule | Why |
|---|---|
| `deny default` | Everything else follows from this |
| `file-map-executable` on system and runtime paths | Recent macOS requires it to load any dylib; without it the interpreter aborts (exit 134) |
| `process-exec` within the interpreter's install root | Homebrew's framework `python3.13` is a launcher that re-executes `Python.app/Contents/MacOS/Python` |
| `file-read-data` on `/` itself | Python lists the root directory during startup |
| `file-read-metadata` everywhere | Python stats many paths at startup. This reveals names and sizes, never contents |

## Probe results

Every attempt in spec 9.3 behaves as required:
- blocked: reading a sentinel, listing home, reading another job, writing outside, external and loopback connections, DNS, spawning `/bin/sh`;
- allowed: reading `in/` and writing `work/`;
- a 256 MB limit kills an allocating worker, reported as `E_RESOURCE_MEMORY`.

Run without the sandbox, the same probe is allowed to do everything, so the results are meaningful.

A real job (the `dev-fake` adapter) runs end to end through the scheduler inside the sandbox.

Startup overhead from `sandbox-exec` plus the runner is under 20 ms.
