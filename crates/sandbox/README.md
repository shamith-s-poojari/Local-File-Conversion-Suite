# converter-sandbox

Worker sandbox (spec section 9). Windows is implemented (T09); Linux and macOS are T10 and currently refuse to launch, so jobs fail with `E_SANDBOX_UNAVAILABLE`. There is no unsandboxed fallback.

## How it fits

The supervisor (`converter-scheduler`) asks a `Launcher` for a `Command`. `SandboxLauncher` returns a command for `sandbox-launch`, a small trusted binary from this crate, with a `SandboxSpec` as its only argument. The shim applies the sandbox, runs the worker on the supervisor's stdio pipes, and exits with the worker's exit code, or `97` (`SANDBOX_SETUP_FAILED_EXIT`) if it couldn't apply the sandbox, which the supervisor reports as `E_SANDBOX_UNAVAILABLE`.

## Windows

| Spec 9.2 requirement | How |
|---|---|
| AppContainer launch | `CreateProcessW` with `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`. One container (and profile) per job: `lfcs.worker.<jobId>` |
| Filesystem | The container SID gets full access to the job directory only. The runtime is readable through the `lfcsWorkerRuntime` capability SID, which only our workers hold |
| Network | No network capabilities, so loopback is blocked too |
| Memory limit | Job Object `JOB_OBJECT_LIMIT_JOB_MEMORY` (admission's hard limit, or 1 GiB before admission) |
| Process limit | Job Object active-process limit; with a limit of 1, the child-process policy also forbids creating children |
| Kill the whole tree | Kill-on-close Job Object, assigned atomically at creation (`PROC_THREAD_ATTRIBUTE_JOB_LIST`); killing the shim closes it |
| Handles | `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` with only stdin, stdout and stderr |
| Environment | Built from scratch: `SystemRoot`, `windir`, `LOCALAPPDATA`, `TEMP`/`TMP`, Python's UTF-8 settings |

Things we learned the hard way:

- **No profile, no process.** `CreateProcessW` fails with "file not found" unless `CreateAppContainerProfile` has registered the container. The shim creates the profile and deletes it when the worker exits. If the shim is killed it can't, so the app must call `windows::remove_stale_profiles(PROFILE_PREFIX)` at startup, after killing leftover workers.
- **`LOCALAPPDATA` is required.** Without it in the environment block, `CreateProcessW` fails with error 203. Windows rewrites it, and `TEMP`, to the container's profile folder. The Python bootstrap points `tempfile` back at the job's `work/`.
- **No console.** A console process gets a `conhost.exe` inside the job, which uses up a process limit of 1 and fails with `STATUS_DLL_INIT_FAILED`. Workers run with `DETACHED_PROCESS`.
- **No venv launcher.** A venv's `python.exe` on Windows starts the real interpreter as a child. `WorkerRuntime::from_venv` uses the base interpreter and adds the venv's `site-packages` through the bootstrap.

### Runtime access

`SandboxLauncher::new` calls `windows::grant_runtime` on the interpreter's directory, the site directories (and the directories their `.pth` files point at), and the adapters directory. It adds one inheritable read and execute ACE for the runtime capability and is a no-op once done. For a per-user install in `%LOCALAPPDATA%`, the installer should do the same (spec 9.2's install-location trap).

## Tests

`tests/windows.rs` runs the real shim with the workspace venv (`uv sync`); without it the tests print a note and pass. `tests/probe.py` is a first cut of the spec 9.3 probe (T11 turns it into the self-test). It checks that the worker can read its job and write `work/`, and that it can't read or list outside the job, read another job's directory, write outside, connect to loopback, resolve DNS, or spawn a process. The other tests cover the memory limit, killing the shim with no orphan left behind, the stale-profile sweep, setup failures and a `dev-fake` job through the `Runner`.

The first run adds the capability ACE to your uv Python install, the venv's `site-packages`, `workers/sdk/src` and `adapters/`. It's a read-only grant to a SID only these workers hold.
