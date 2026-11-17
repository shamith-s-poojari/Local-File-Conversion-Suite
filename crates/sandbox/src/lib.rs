//! Per-job OS sandbox for workers (spec section 9).
//!
//! A worker is started as
//!
//! ```text
//! converter-sandbox-run --memory-limit N --job-id J -- <OS sandbox> python -I adapter.py J
//! ```
//!
//! - The **OS sandbox** confines the worker: read-only runtime and adapters,
//!   read-write job directory, nothing else, no network. On macOS that is a
//!   per-job Seatbelt profile (spike S1 / change C8); on Linux bubblewrap
//!   (C7); on Windows an AppContainer and Job Object that the runner sets up
//!   itself (T09, see `windows.rs`). Anywhere else [`SandboxLauncher`]
//!   refuses and jobs fail with `E_SANDBOX_UNAVAILABLE`: never unsandboxed.
//! - The **runner** (`converter-sandbox-run`, outside the OS sandbox)
//!   enforces the memory limit with an RSS watchdog. On a breach it kills
//!   the worker and writes a protocol `error` line with `E_RESOURCE_MEMORY`,
//!   so the scheduler reports the right code without knowing about it.
//!
//! The environment is scrubbed and rebuilt; the worker's working directory
//! is the job directory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use converter_scheduler::{LaunchSpec, Launcher};

#[cfg(any(target_os = "linux", all(unix, test)))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod probe;
pub mod runner;
#[cfg(windows)]
mod windows;

/// Memory ceiling before admission (validate and estimate), when the
/// scheduler hasn't set one.
pub const DEFAULT_MEMORY_LIMIT: u64 = 1024 * 1024 * 1024;

/// Where the worker runtime lives. Built once at startup, from the bundled
/// interpreter in a release or the workspace virtualenv in development.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// The interpreter to run, e.g. `.venv/bin/python`.
    pub python: PathBuf,
    /// Symlinks met on the way from `python` to the real interpreter, as
    /// (link, target). Bubblewrap mounts only resolved folders, so it
    /// recreates these; uv's `.venv/bin/python` points through a
    /// `cpython-3.13-…` link to `cpython-3.13.x-…`.
    pub python_links: Vec<(PathBuf, PathBuf)>,
    /// Directories the worker may read: interpreter, libraries, site-packages,
    /// and every `sys.path` entry (editable installs live outside site-packages).
    pub runtime_read: Vec<PathBuf>,
    /// Directories whose binaries the worker may execute (the interpreter's
    /// own prefix, so `pdf-redact` can start its verifier).
    pub exec_roots: Vec<PathBuf>,
    /// `adapters/`, holding one folder per adapter.
    pub adapters_dir: PathBuf,
    /// The runner: `converter-sandbox-run`, or the app's own executable.
    pub runner: PathBuf,
    /// Arguments before the runner's own, e.g. `--sandbox-run` for the app.
    pub runner_args: Vec<OsString>,
    /// Processes the interpreter itself starts: on Windows a virtualenv's
    /// `python.exe` is a launcher that runs the real interpreter as a child.
    pub interpreter_processes: u32,
    /// Extra access for adapters that run an external engine (ADR 0003),
    /// by adapter ID. Adapters not listed get none.
    pub grants: BTreeMap<String, Grant>,
}

/// What an external engine needs inside the sandbox: its install read-only
/// (and mappable), its binaries executable, and where they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    /// Directories to read and map, e.g. the engine's install.
    pub read: Vec<PathBuf>,
    /// Binaries (or directories of them) the worker may execute.
    pub exec: Vec<PathBuf>,
    /// Environment for the worker, e.g. `LFCS_BIN_FFMPEG=/…/ffmpeg`.
    pub env: Vec<(String, OsString)>,
    /// macOS only: Mach services the engine must look up (fonts, preferences).
    pub mach: Vec<String>,
    /// macOS only: Unix sockets the engine creates and listens on, as a
    /// directory and a regex for the socket's name in it. The profile allows
    /// creating them and accepting connections, plus write on the directory
    /// itself (LibreOffice checks `access("/tmp", W_OK)` before making its
    /// single-instance pipe there; a root-owned sticky directory stays
    /// unchangeable either way). Connecting out is never allowed, so an
    /// engine can't reach another program's socket. Linux gives each job a
    /// private `/tmp` instead.
    pub listen: Vec<(PathBuf, String)>,
    /// Processes the worker may run besides itself, at once (an engine and
    /// its helpers). Enforced on Windows by the Job Object (spec 9.2:
    /// "1 for PDF workers; set per engine").
    pub processes: u32,
}

impl Grant {
    /// Combine two grants (an adapter needing two engines).
    pub fn merge(&mut self, other: &Grant) {
        for (mine, theirs) in [(&mut self.read, &other.read), (&mut self.exec, &other.exec)] {
            for p in theirs {
                if !mine.contains(p) {
                    mine.push(p.clone());
                }
            }
        }
        for e in &other.env {
            if !self.env.contains(e) {
                self.env.push(e.clone());
            }
        }
        for m in &other.mach {
            if !self.mach.contains(m) {
                self.mach.push(m.clone());
            }
        }
        for l in &other.listen {
            if !self.listen.contains(l) {
                self.listen.push(l.clone());
            }
        }
        // Adapters run their engines one at a time.
        self.processes = self.processes.max(other.processes);
    }
}

impl SandboxConfig {
    /// Asks `python` where its runtime lives. Runs our own interpreter,
    /// unsandboxed, with no input: this is configuration, not a job.
    pub fn discover(python: &Path, adapters_dir: &Path, runner: &Path) -> Result<Self, String> {
        Self::discover_within(python, adapters_dir, runner, DISCOVER_TIMEOUT)
    }

    fn discover_within(
        python: &Path,
        adapters_dir: &Path,
        runner: &Path,
        timeout: Duration,
    ) -> Result<Self, String> {
        // The folder resolved, the file name kept: a virtualenv is found from
        // the path the interpreter is started by, and `crates/x/../..` names
        // folders a sandbox doesn't have.
        let python = &match (python.parent(), python.file_name()) {
            (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => canonical(dir)?.join(name),
            _ => python.to_path_buf(),
        };
        let mut cmd = Command::new(python);
        cmd.args([
            "-I",
            "-c",
            "import os, sys, sysconfig\n\
                 paths = {sys.prefix, sys.base_prefix, sys.exec_prefix, sys.base_exec_prefix,\n\
                          os.path.dirname(os.path.realpath(sys.executable)),\n\
                          sysconfig.get_paths()['purelib'], sysconfig.get_paths()['platlib']}\n\
                 paths |= {p for p in sys.path if p and os.path.isdir(p)}\n\
                 print(os.path.realpath(sys.executable))\n\
                 print(os.path.realpath(sys.base_prefix))\n\
                 for p in sorted(paths): print(os.path.realpath(p))",
        ])
        .env_clear()
        .envs(base_env());
        let stdout = run_bounded(cmd, timeout)?;
        let text = String::from_utf8(stdout).map_err(|_| "non-UTF-8 runtime path")?;
        let mut lines = text.lines().map(PathBuf::from);
        let executable = lines.next().ok_or("no executable path")?;
        let base = lines.next().ok_or("no base prefix")?;
        let mut runtime_read: Vec<PathBuf> = lines.collect();
        // A virtualenv's own directory (pyvenv.cfg, bin/) sits above site-packages.
        if let Some(venv) = python.parent().and_then(Path::parent) {
            runtime_read.push(canonical(venv)?);
        }
        let exec_root = installation_root(&executable, &base)?;
        runtime_read.push(exec_root.clone());
        runtime_read.sort();
        runtime_read.dedup();
        let venv_launcher = cfg!(windows)
            && python
                .parent()
                .and_then(Path::parent)
                .is_some_and(|v| v.join("pyvenv.cfg").is_file());
        Ok(SandboxConfig {
            python: python.to_path_buf(),
            python_links: symlink_chain(python),
            runtime_read,
            exec_roots: vec![exec_root],
            adapters_dir: canonical(adapters_dir)?,
            runner: runner.to_path_buf(),
            runner_args: Vec::new(),
            interpreter_processes: u32::from(venv_launcher),
            grants: BTreeMap::new(),
        })
    }

    /// The runtime and adapters: every worker reads them.
    pub fn runtime_roots(&self) -> Vec<&Path> {
        let mut out: Vec<&Path> = self
            .runtime_read
            .iter()
            .chain(&self.exec_roots)
            .chain(std::iter::once(&self.adapters_dir))
            .map(PathBuf::as_path)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Engine installs that some adapters' workers read.
    pub fn engine_roots(&self) -> Vec<&Path> {
        let mut out: Vec<&Path> = Vec::new();
        for g in self.grants.values() {
            out.extend(g.read.iter().map(PathBuf::as_path));
            out.extend(g.exec.iter().filter(|p| p.is_dir()).map(PathBuf::as_path));
        }
        out.sort();
        out.dedup();
        out
    }

    fn adapter_script(&self, adapter_id: &str) -> Result<PathBuf, String> {
        let valid = !adapter_id.is_empty()
            && adapter_id.len() <= 64
            && adapter_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !adapter_id.starts_with('-');
        if !valid {
            return Err("invalid adapter id".into());
        }
        let script = self.adapters_dir.join(adapter_id).join("adapter.py");
        if !script.is_file() {
            return Err("adapter not installed".into());
        }
        Ok(script)
    }
}

/// How long the interpreter gets to describe itself. Startup and the
/// self-test wait on it; a hung interpreter must fail them, not freeze the app.
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(20);

/// Runs `cmd` with no input and returns its stdout, or an error if it fails
/// or outlives `timeout` (then it is killed). Stdout is drained on a thread
/// so a large answer can't fill the pipe and look like a hang.
fn run_bounded(mut cmd: Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("can't run the worker interpreter: {e}"))?;
    let mut pipe = child.stdout.take().ok_or("no output pipe")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the worker interpreter did not answer in time".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("can't run the worker interpreter: {e}")),
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        return Err("the worker interpreter failed to describe itself".into());
    }
    Ok(out)
}

/// The interpreter's installation root: for a Homebrew framework build the
/// whole keg (the `python3.13` launcher re-executes `Python.app` inside
/// it), for any other framework build its `Python.framework/Versions/X`,
/// otherwise the base prefix. Never a folder that is, or contains, the home
/// folder or `/`: a python.org install lives under `/Library/Frameworks`, and
/// granting `/Library` would hand the worker every installed framework,
/// font and preference file.
fn installation_root(executable: &Path, base: &Path) -> Result<PathBuf, String> {
    installation_root_in(executable, base, home_dir().as_deref())
}

fn installation_root_in(
    executable: &Path,
    base: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    // e.g. /opt/homebrew/Cellar/python@3.13/3.13.4/Frameworks/Python.framework/…
    let keg = executable.ancestors().find(|a| {
        a.parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n == "Cellar")
    });
    // e.g. /Library/Frameworks/Python.framework/Versions/3.13
    let framework = executable.ancestors().find(|a| {
        a.parent().is_some_and(|v| {
            v.file_name().is_some_and(|n| n == "Versions")
                && v.parent()
                    .and_then(Path::extension)
                    .is_some_and(|e| e == "framework")
        })
    });
    for candidate in [keg, framework, Some(base)].into_iter().flatten() {
        if !too_broad(candidate, home) {
            return Ok(candidate.to_path_buf());
        }
    }
    Err("the interpreter's install folder is too broad to share with workers".into())
}

/// The current user's home or profile folder, resolved.
pub fn home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())?;
    Some(home.canonicalize().map(|c| plain_path(&c)).unwrap_or(home))
}

/// Folders at the top of a drive that hold other people's or the system's
/// files. Matched without regard to case, for Windows.
const BROAD_TOP_LEVEL: &[&str] = &[
    "applications",
    "bin",
    "dev",
    "etc",
    "home",
    "lib",
    "library",
    "opt",
    "private",
    "proc",
    "programdata",
    "program files",
    "program files (x86)",
    "root",
    "sbin",
    "system",
    "tmp",
    "users",
    "var",
    "volumes",
    "windows",
];

/// Whether `outer` is `inner` or one of its parents (case-insensitive on
/// Windows, where `Path::starts_with` is not).
fn encloses(outer: &Path, inner: &Path) -> bool {
    if !cfg!(windows) {
        return inner.starts_with(outer);
    }
    let mut inner = inner.components();
    outer.components().all(|o| {
        inner
            .next()
            .is_some_and(|i| i.as_os_str().eq_ignore_ascii_case(o.as_os_str()))
    })
}

/// Whether granting `path` would expose far more than an install: a relative
/// path, a drive root or `/`, the home folder or any folder holding it, or a
/// system folder such as `/Library`, `/Users` or `C:\Windows`. Engine and
/// interpreter installs are shared with workers read-only; these must never
/// be, however the binary was found (`~/bin/ffmpeg` is in `PATH` for many
/// people).
pub fn too_broad(path: &Path, home: Option<&Path>) -> bool {
    if !path.is_absolute() {
        return true;
    }
    let names: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n),
            _ => None,
        })
        .collect();
    if names.is_empty() || home.is_some_and(|h| encloses(path, h)) {
        return true;
    }
    names.len() == 1
        && BROAD_TOP_LEVEL
            .iter()
            .any(|b| names[0].eq_ignore_ascii_case(b))
}

/// Whether the current user owns the file or folder (`lstat`: a symlink is
/// judged by itself). Windows has no owner uid to compare here, so the check
/// holds there; its development checkouts sit in the user's own profile.
pub fn owned_by_current_user(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        meta.uid() == unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        true
    }
}

/// The symlinks met while resolving `path`, in order, as (link, target)
/// with absolute targets. Gives up quietly on a loop or an unreadable link:
/// the sandbox then reports the start failure as before.
fn symlink_chain(path: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut links = Vec::new();
    let mut pending: Vec<std::ffi::OsString> = path
        .components()
        .rev()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    let mut at = PathBuf::new();
    while let Some(part) = pending.pop() {
        if part == ".." {
            at.pop();
            continue;
        }
        let next = at.join(&part);
        match std::fs::symlink_metadata(&next) {
            Ok(m) if m.file_type().is_symlink() => {
                let Ok(target) = std::fs::read_link(&next) else {
                    return links;
                };
                let target = if target.is_absolute() {
                    target
                } else {
                    at.join(target)
                };
                links.push((next, target.clone()));
                if links.len() > 40 {
                    return Vec::new();
                }
                // Resolve the target from the root again.
                pending.extend(
                    target
                        .components()
                        .rev()
                        .map(|c| c.as_os_str().to_os_string()),
                );
                at = PathBuf::new();
            }
            _ => at = next,
        }
    }
    links
}

fn canonical(p: &Path) -> Result<PathBuf, String> {
    p.canonicalize()
        .map(|c| plain_path(&c))
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// On Windows, `canonicalize` gives `\\?\C:\…`; drive paths without that
/// prefix are what interpreters, engines and file URLs expect. Paths that
/// need it (too long, or UNC) keep it.
pub fn plain_path(p: &Path) -> PathBuf {
    if cfg!(windows) {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && b[2] == b'\\'
                && rest.len() < 260
            {
                return PathBuf::from(rest);
            }
        }
    }
    p.to_path_buf()
}

/// What the runner executes inside the OS sandbox, before platform wrapping.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) struct Inner<'a> {
    pub config: &'a SandboxConfig,
    pub job_dir: PathBuf,
    pub argv: Vec<OsString>,
    pub grant: Option<&'a Grant>,
}

/// The scheduler's launcher (spec 9.1 `spawn`).
#[derive(Debug, Clone)]
pub struct SandboxLauncher {
    config: SandboxConfig,
}

impl SandboxLauncher {
    pub fn new(config: SandboxConfig) -> Self {
        SandboxLauncher { config }
    }

    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// One-time setup before the first worker. On Windows, makes the
    /// runtime and engine installs readable by AppContainers (spec 9.2's
    /// install-location trap: a per-user install isn't, Program Files is).
    /// Nothing to do elsewhere.
    ///
    /// The runtime must succeed (no worker starts without it); engines are
    /// best effort, since only their own conversions need them.
    pub fn prepare(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            windows::prepare(&self.config.runtime_roots())?;
            let _ = windows::prepare(&self.config.engine_roots());
        }
        Ok(())
    }

    /// Runs `argv` (interpreter first) in the job's sandbox, under the runner.
    /// Shared by workers and the probe, which gets no engine grant.
    pub fn wrap(
        &self,
        job_id: &str,
        job_dir: &Path,
        memory_limit: u64,
        argv: Vec<OsString>,
    ) -> Result<Command, String> {
        self.wrap_with(job_id, job_dir, memory_limit, argv, None)
    }

    fn wrap_with(
        &self,
        job_id: &str,
        job_dir: &Path,
        memory_limit: u64,
        argv: Vec<OsString>,
        grant: Option<&Grant>,
    ) -> Result<Command, String> {
        let job_dir = canonical(job_dir)?;
        let inner = Inner {
            config: &self.config,
            job_dir: job_dir.clone(),
            argv,
            grant,
        };
        let sandboxed = platform_command(&inner)?;
        let processes = 1 + self.config.interpreter_processes + grant.map_or(0, |g| g.processes);
        let mut cmd = Command::new(&self.config.runner);
        cmd.args(&self.config.runner_args)
            .arg("--memory-limit")
            .arg(memory_limit.to_string())
            .arg("--job-id")
            .arg(job_id)
            .arg("--max-processes")
            .arg(processes.to_string())
            .arg("--")
            .args(sandboxed);
        cmd.current_dir(&job_dir);
        cmd.env_clear();
        for (key, value) in worker_env(&job_dir) {
            cmd.env(key, value);
        }
        for (key, value) in grant.map(|g| g.env.as_slice()).unwrap_or_default() {
            cmd.env(key, value);
        }
        Ok(cmd)
    }
}

impl Launcher for SandboxLauncher {
    fn command(&self, spec: &LaunchSpec) -> Result<Command, String> {
        let script = self.config.adapter_script(&spec.adapter_id)?;
        let argv = vec![
            self.config.python.clone().into_os_string(),
            "-I".into(),
            script.into_os_string(),
            spec.job_id.as_str().into(),
        ];
        self.wrap_with(
            spec.job_id.as_str(),
            &spec.job_dir,
            spec.memory_limit.unwrap_or(DEFAULT_MEMORY_LIMIT),
            argv,
            self.config.grants.get(&spec.adapter_id),
        )
    }
}

/// What any program needs to start after `env_clear`: on Windows,
/// `SYSTEMROOT` (system DLLs and the random number source); nothing elsewhere.
pub fn base_env() -> Vec<(&'static str, OsString)> {
    if cfg!(windows) {
        let root = std::env::var_os("SYSTEMROOT").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        vec![("SYSTEMROOT", root)]
    } else {
        Vec::new()
    }
}

/// The scrubbed, rebuilt environment (spec 9.2).
pub(crate) fn worker_env(job_dir: &Path) -> Vec<(&'static str, OsString)> {
    let work = job_dir.join("work").into_os_string();
    let mut env = vec![
        ("LANG", OsString::from("C.UTF-8")),
        ("LC_ALL", OsString::from("C.UTF-8")),
        ("TMPDIR", work.clone()),
        ("HOME", work.clone()),
        ("OMP_THREAD_LIMIT", OsString::from("1")),
        ("PYTHONDONTWRITEBYTECODE", OsString::from("1")),
        // Text files default to UTF-8 on every OS, not the Windows code page.
        ("PYTHONUTF8", OsString::from("1")),
    ];
    if cfg!(windows) {
        // Windows finds temp folders through TEMP and TMP, and system DLLs
        // through SYSTEMROOT. Starting an AppContainer process also reads
        // the profile folders to set up the container's own, and fails with
        // error 203 (variable not found) without them; they point at the
        // job's work folder so no real profile path reaches the worker.
        env.push(("TEMP", work.clone()));
        env.push(("TMP", work.clone()));
        env.push(("USERPROFILE", work.clone()));
        env.push(("LOCALAPPDATA", work.clone()));
        env.push(("APPDATA", work));
        let root = std::env::var_os("SYSTEMROOT").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        let mut path = PathBuf::from(&root).join("System32").into_os_string();
        path.push(";");
        path.push(&root);
        env.push(("PATH", path));
        env.push(("SYSTEMROOT", root));
    } else {
        env.push(("PATH", OsString::from("/usr/bin:/bin")));
    }
    env
}

#[cfg(target_os = "macos")]
fn platform_command(inner: &Inner<'_>) -> Result<Vec<OsString>, String> {
    macos::command(inner)
}

#[cfg(target_os = "linux")]
fn platform_command(inner: &Inner<'_>) -> Result<Vec<OsString>, String> {
    linux::command(inner)
}

/// The runner itself starts the worker in an AppContainer (see `windows.rs`).
#[cfg(windows)]
fn platform_command(inner: &Inner<'_>) -> Result<Vec<OsString>, String> {
    Ok(inner.argv.clone())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn platform_command(_: &Inner<'_>) -> Result<Vec<OsString>, String> {
    // The spec forbids unsandboxed workers.
    Err("no sandbox for this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix layouts: "/usr" isn't an absolute path on Windows.
    #[cfg(unix)]
    #[test]
    fn framework_installs_use_the_whole_versioned_root() {
        let exe = Path::new(
            "/opt/homebrew/Cellar/python@3.13/3.13.4/Frameworks/Python.framework/Versions/3.13/bin/python3.13",
        );
        let base = Path::new(
            "/opt/homebrew/Cellar/python@3.13/3.13.4/Frameworks/Python.framework/Versions/3.13",
        );
        let home = Path::new("/Users/me");
        assert_eq!(
            installation_root_in(exe, base, Some(home)).unwrap(),
            Path::new("/opt/homebrew/Cellar/python@3.13/3.13.4")
        );
        assert_eq!(
            installation_root_in(
                Path::new("/usr/bin/python3.12"),
                Path::new("/usr"),
                Some(home)
            )
            .unwrap(),
            Path::new("/usr")
        );
    }

    // Unix layouts: "/usr" isn't an absolute path on Windows.
    #[cfg(unix)]
    #[test]
    fn python_org_framework_grants_its_version_not_library() {
        let home = Path::new("/Users/me");
        let base = Path::new("/Library/Frameworks/Python.framework/Versions/3.13");
        let exe = base.join("bin/python3.13");
        assert_eq!(installation_root_in(&exe, base, Some(home)).unwrap(), base);
    }

    // Unix layouts: "/usr" isn't an absolute path on Windows.
    #[cfg(unix)]
    #[test]
    fn an_install_that_is_or_holds_home_is_refused() {
        let home = Path::new("/Users/me");
        // A prefix that is the home folder, one that contains it, and `/`.
        for base in ["/Users/me", "/Users", "/"] {
            let base = Path::new(base);
            assert!(
                installation_root_in(&base.join("bin/python3"), base, Some(home)).is_err(),
                "{base:?}"
            );
        }
        // Inside home is fine (uv-managed interpreters live there).
        let uv = Path::new("/Users/me/.local/share/uv/python/cpython-3.13");
        assert_eq!(
            installation_root_in(&uv.join("bin/python3"), uv, Some(home)).unwrap(),
            uv
        );
    }

    // Unix layouts: "/usr" isn't an absolute path on Windows.
    #[cfg(unix)]
    #[test]
    fn too_broad_names_the_dangerous_roots() {
        let home = Some(Path::new("/Users/me"));
        for p in ["/", "/Users", "/Users/me", "/Library", "/home", "rel/dir"] {
            assert!(too_broad(Path::new(p), home), "{p}");
        }
        for p in [
            "/usr",
            "/opt/homebrew/Cellar",
            "/Users/me/bin",
            "/Applications/LibreOffice.app",
        ] {
            assert!(!too_broad(Path::new(p), home), "{p}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn ownership_is_the_current_users() {
        let t = tempfile::tempdir().unwrap();
        let meta = std::fs::metadata(t.path()).unwrap();
        assert!(owned_by_current_user(&meta));
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_interpreter_fails_discovery_instead_of_hanging() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let fake = t.path().join("python");
        // exec: the sleep is the child, so killing it closes the pipe.
        std::fs::write(&fake, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        let e = SandboxConfig::discover_within(
            &fake,
            t.path(),
            Path::new("/x"),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(e.contains("did not answer in time"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn adapter_ids_cannot_escape() {
        let config = SandboxConfig {
            python_links: Vec::new(),
            python: "/x".into(),
            runtime_read: vec![],
            exec_roots: vec![],
            adapters_dir: "/nonexistent".into(),
            runner: "/x".into(),
            runner_args: vec![],
            interpreter_processes: 0,
            grants: BTreeMap::new(),
        };
        for bad in ["../x", "a/b", "", "-x", "A", "a.b"] {
            assert!(config.adapter_script(bad).is_err(), "{bad}");
        }
    }
}
