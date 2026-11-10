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
//!   (C7). Windows (AppContainer, T09) isn't implemented yet, so
//!   [`SandboxLauncher`] refuses there and jobs fail with
//!   `E_SANDBOX_UNAVAILABLE`, as the spec requires: never unsandboxed.
//! - The **runner** (`converter-sandbox-run`, outside the OS sandbox)
//!   enforces the memory limit with an RSS watchdog. On a breach it kills
//!   the worker and writes a protocol `error` line with `E_RESOURCE_MEMORY`,
//!   so the scheduler reports the right code without knowing about it.
//!
//! The environment is scrubbed and rebuilt; the worker's working directory
//! is the job directory.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use converter_scheduler::{LaunchSpec, Launcher};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod probe;
pub mod runner;

/// Memory ceiling before admission (validate and estimate), when the
/// scheduler hasn't set one.
pub const DEFAULT_MEMORY_LIMIT: u64 = 1024 * 1024 * 1024;

/// Where the worker runtime lives. Built once at startup, from the bundled
/// interpreter in a release or the workspace virtualenv in development.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// The interpreter to run, e.g. `.venv/bin/python`.
    pub python: PathBuf,
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
}

impl SandboxConfig {
    /// Asks `python` where its runtime lives. Runs our own interpreter,
    /// unsandboxed, with no input: this is configuration, not a job.
    pub fn discover(python: &Path, adapters_dir: &Path, runner: &Path) -> Result<Self, String> {
        let out = Command::new(python)
            .args([
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
            .output()
            .map_err(|e| format!("can't run the worker interpreter: {e}"))?;
        if !out.status.success() {
            return Err("the worker interpreter failed to describe itself".into());
        }
        let text = String::from_utf8(out.stdout).map_err(|_| "non-UTF-8 runtime path")?;
        let mut lines = text.lines().map(PathBuf::from);
        let executable = lines.next().ok_or("no executable path")?;
        let base = lines.next().ok_or("no base prefix")?;
        let mut runtime_read: Vec<PathBuf> = lines.collect();
        // A virtualenv's own directory (pyvenv.cfg, bin/) sits above site-packages.
        if let Some(venv) = python.parent().and_then(Path::parent) {
            runtime_read.push(canonical(venv)?);
        }
        let exec_root = installation_root(&executable, &base);
        runtime_read.push(exec_root.clone());
        runtime_read.sort();
        runtime_read.dedup();
        Ok(SandboxConfig {
            python: python.to_path_buf(),
            runtime_read,
            exec_roots: vec![exec_root],
            adapters_dir: canonical(adapters_dir)?,
            runner: runner.to_path_buf(),
            runner_args: Vec::new(),
        })
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

/// The interpreter's installation root: for a framework build the whole
/// versioned install (the `python3.13` launcher re-executes `Python.app`
/// inside it), otherwise the base prefix.
fn installation_root(executable: &Path, base: &Path) -> PathBuf {
    // e.g. /opt/homebrew/Cellar/python@3.13/3.13.4/Frameworks/Python.framework/…
    let mut root = base.to_path_buf();
    for ancestor in executable.ancestors() {
        if ancestor.file_name().is_some_and(|n| n == "Frameworks") {
            if let Some(parent) = ancestor.parent() {
                root = parent.to_path_buf();
            }
            break;
        }
    }
    root
}

fn canonical(p: &Path) -> Result<PathBuf, String> {
    p.canonicalize()
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// What the runner executes inside the OS sandbox, before platform wrapping.
// Unused until Windows has a sandbox (T09).
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) struct Inner<'a> {
    pub config: &'a SandboxConfig,
    pub job_dir: PathBuf,
    pub argv: Vec<OsString>,
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

    /// Runs `argv` (interpreter first) in the job's sandbox, under the runner.
    /// Shared by workers and the probe.
    pub fn wrap(
        &self,
        job_id: &str,
        job_dir: &Path,
        memory_limit: u64,
        argv: Vec<OsString>,
    ) -> Result<Command, String> {
        let job_dir = canonical(job_dir)?;
        let inner = Inner {
            config: &self.config,
            job_dir: job_dir.clone(),
            argv,
        };
        let sandboxed = platform_command(&inner)?;
        let mut cmd = Command::new(&self.config.runner);
        cmd.args(&self.config.runner_args)
            .arg("--memory-limit")
            .arg(memory_limit.to_string())
            .arg("--job-id")
            .arg(job_id)
            .arg("--")
            .args(sandboxed);
        cmd.current_dir(&job_dir);
        cmd.env_clear();
        for (key, value) in worker_env(&job_dir) {
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
        self.wrap(
            spec.job_id.as_str(),
            &spec.job_dir,
            spec.memory_limit.unwrap_or(DEFAULT_MEMORY_LIMIT),
            argv,
        )
    }
}

/// The scrubbed, rebuilt environment (spec 9.2).
fn worker_env(job_dir: &Path) -> Vec<(&'static str, OsString)> {
    let work = job_dir.join("work").into_os_string();
    let mut env = vec![
        ("PATH", OsString::from("/usr/bin:/bin")),
        ("LANG", OsString::from("C.UTF-8")),
        ("LC_ALL", OsString::from("C.UTF-8")),
        ("TMPDIR", work.clone()),
        ("HOME", work),
        ("OMP_THREAD_LIMIT", OsString::from("1")),
        ("PYTHONDONTWRITEBYTECODE", OsString::from("1")),
    ];
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SYSTEMROOT") {
            env.push(("SYSTEMROOT", root));
        }
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_command(_: &Inner<'_>) -> Result<Vec<OsString>, String> {
    // T09: AppContainer. Until then, refuse: the spec forbids unsandboxed workers.
    Err("no sandbox for this platform yet".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_installs_use_the_whole_versioned_root() {
        let exe = Path::new(
            "/opt/homebrew/Cellar/python@3.13/3.13.4/Frameworks/Python.framework/Versions/3.13/bin/python3.13",
        );
        let base = Path::new(
            "/opt/homebrew/Cellar/python@3.13/3.13.4/Frameworks/Python.framework/Versions/3.13",
        );
        assert_eq!(
            installation_root(exe, base),
            Path::new("/opt/homebrew/Cellar/python@3.13/3.13.4")
        );
        assert_eq!(
            installation_root(Path::new("/usr/bin/python3.12"), Path::new("/usr")),
            Path::new("/usr")
        );
    }

    #[test]
    fn adapter_ids_cannot_escape() {
        let config = SandboxConfig {
            python: "/x".into(),
            runtime_read: vec![],
            exec_roots: vec![],
            adapters_dir: "/nonexistent".into(),
            runner: "/x".into(),
            runner_args: vec![],
        };
        for bad in ["../x", "a/b", "", "-x", "A", "a.b"] {
            assert!(config.adapter_script(bad).is_err(), "{bad}");
        }
    }
}
