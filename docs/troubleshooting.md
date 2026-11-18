# Troubleshooting

The command line is the quickest way to see what the app sees. From a checkout:

```bash
LFCS_DEV=1 cargo run -p lfcs -- doctor
```

`lfcs doctor` runs the sandbox self-test, prints each check as `ok` or `FAIL`, then lists the engines it found and the adapters it loaded. It exits with code 3 if the self-test failed. The app's Settings page shows the same self-test result.

## The sandbox self-test fails ("Processing is turned off")

At every start the app runs a self-test inside a fresh sandbox: it must be able to read its own runtime and write its job folder, and it must be refused everything else (your home folder, other jobs, writing outside the job, network connections including loopback and name lookups, starting a shell, hard links out of the job, a memory limit). If any check doesn't give the expected answer, or the sandbox can't start at all, **processing stays off**. The app never runs a conversion without the sandbox, and there is no setting to override that.

Read the `FAIL` lines of `lfcs doctor`:

- `runtime_readable`, or an error before any check: the sandbox couldn't start or couldn't read the Python runtime. See the system sections below.
- A `blocked` check that shows `allowed` (for example `connect_external`, `write_outside`): the sandbox starts but isn't confining. Don't work around it; report it with the full `doctor` output.
- `memory_limit`: the system didn't enforce the memory cap.

### macOS

The sandbox uses `/usr/bin/sandbox-exec` (deprecated by Apple, but present). If it is missing or blocked by a device-management profile, the self-test fails. Running the app from inside another sandbox (some terminals and IDE containers) can also stop it from starting its own.

### Linux

The sandbox uses **bubblewrap** (`bwrap`), which needs **unprivileged user namespaces**.

1. Install it: `sudo apt-get install bubblewrap` (Debian and Ubuntu), `sudo dnf install bubblewrap` (Fedora). Versions 0.8 and newer add a block on nested user namespaces; older versions work without it.
2. Check that it can start a sandbox: `bwrap --unshare-all --ro-bind / / true`. If that prints an error such as `setting up uid map: Permission denied` or `Operation not permitted`, user namespaces are restricted:
   - **Ubuntu 24.04 and later** restrict them with AppArmor. For a machine you control, `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` lifts it until reboot (put the setting in a file under `/etc/sysctl.d/` to keep it). That lifts a protection for every program on the machine, so it is your call; CI does it on throwaway runners. An AppArmor profile that allows only `bwrap` is the narrower fix. This repository doesn't ship one.
   - **Debian** may need `sudo sysctl -w kernel.unprivileged_userns_clone=1`.
   - A **container** (Docker, some CI systems, Flatpak or Snap confinement) often blocks nested user namespaces. Run the app on the host, or allow the container to create them.
   - `user.max_user_namespaces` set to 0 also blocks them: `sysctl user.max_user_namespaces`.
3. Run `lfcs doctor` again.

The Linux sandbox also applies a seccomp filter that refuses a list of system calls a converter never needs (ptrace, mounts, namespaces, kernel modules, BPF, and similar). It exists for x86-64 and aarch64 only; on another architecture the sandbox runs without it, on the namespaces alone.

### Windows

The sandbox uses an AppContainer and a Job Object. It has only been run on GitHub's Windows runners, not on a range of real machines. If the self-test fails, run `lfcs doctor` and keep the output. Endpoint-protection software that blocks AppContainer processes, and a data folder inside OneDrive or another synced folder (the app refuses those), are the first things to rule out.

## An engine is missing

Audio and video need FFmpeg; Word, Excel, PowerPoint and OpenDocument files need LibreOffice. Without them those conversions are shown as unavailable, with the install command for the system. Install it and **reopen the app**: engines are looked for once, at start-up.

| System | FFmpeg | LibreOffice |
|---|---|---|
| macOS | `brew install ffmpeg-full` | `brew install --cask libreoffice` |
| Windows | `winget install Gyan.FFmpeg` (or `choco install ffmpeg`, or Scoop) | `winget install TheDocumentFoundation.LibreOffice` |
| Linux | `sudo apt install ffmpeg` | `sudo apt install libreoffice` |

If it is installed and the app still doesn't see it:

- `lfcs doctor` prints an `engines:` line. FFmpeg needs both `ffmpeg` and `ffprobe`; LibreOffice shows as `libreoffice`.
- The app looks only in the places listed in [Engines](engines.md). LibreOffice isn't looked for on `PATH`, so a Snap or Flatpak LibreOffice, or one in an unusual folder, isn't found.
- A program that sits directly in your home folder, or in a folder whose sharing would expose it (`~/ffmpeg`), is treated as not installed. Put it in a folder of its own.
- On Windows, a Chocolatey shim isn't used; the app looks in the package's own folder instead.

### Ogg files fail with an "install is corrupt" message

Writing Ogg (Vorbis) needs an FFmpeg built with libvorbis, and Homebrew's plain `ffmpeg` isn't. Use `brew install ffmpeg-full`. The app looks in its folder, but `PATH` is searched first: if a plain `ffmpeg` is earlier on your `PATH` and you start the app from a terminal, that one is picked. Check with `which ffmpeg`, or run `brew unlink ffmpeg` (or put `ffmpeg-full`'s `bin` first).

## LibreOffice conversions

- **First conversion is slow.** LibreOffice builds a fresh profile for every job. A large document can take a few minutes; a document that takes longer than 300 seconds is stopped.
- **Exit code 81 (restart).** On a new profile LibreOffice quits with code 81 and expects to be started again. The `soffice` launcher normally handles that; the app runs LibreOffice's binary directly, so the worker restarts it once itself. This is handled and needs no action. Builds from before this fix fail the first Office conversion on a fresh profile; update if you see exit code 81.
- **A file "could not be opened" (`E_INPUT_CORRUPT`).** LibreOffice writes nothing for a damaged file, and for one that needs a password (no password is passed to it). Try opening the file in LibreOffice itself.
- **Layout or fonts differ.** Fonts the system lacks are replaced. Install the fonts the document uses.

## "The resources folder has no worker runtime" or "can't find the converter's files"

The app and `lfcs` need a Python runtime. A release build carries its own in the resources folder (`python/`, `adapters/`, `engines/`; see [Release plan](release-plan.md)). From a checkout there is no bundled runtime, so development mode is needed, and it is opt-in:

```bash
uv sync
LFCS_DEV=1 pnpm dev
```

or `lfcs --dev …`. Development mode uses the checkout's `.venv`, and only if it sits inside the repository and is owned by you. The core never looks for a runtime in the current folder. If `uv sync` hasn't been run you get "the worker runtime is missing (run `uv sync` in development)".

To try a runtime built by `tools/bundle-runtime`, pass it as `--resources`: `lfcs doctor --resources target/bundle-runtime`.

## Where else to look

- [Known limits](known-limits.md) for what the tools can't do.
- [Engines](engines.md) for where FFmpeg and LibreOffice are looked for.
- [Privacy](privacy.md) and the [threat model](threat-model.md) for what the sandbox covers.
