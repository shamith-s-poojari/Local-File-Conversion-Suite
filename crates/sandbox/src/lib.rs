//! Worker sandbox (spec section 9).
//!
//! The core never starts a worker directly. It runs the `sandbox-launch`
//! shim, a small trusted binary from this crate, and hands it a
//! [`SandboxSpec`] as one JSON argument. The shim applies the OS sandbox,
//! starts the worker with the core's stdio pipes, waits for it, and exits
//! with its exit code. Keeping the setup in a separate process lets the
//! supervisor keep using a plain [`std::process::Command`], and killing the
//! shim's Job Object kills the worker with it.
//!
//! Windows (T09): see [`windows`] for what the shim enforces. Linux and
//! macOS (T10) aren't implemented yet: [`SandboxLauncher::command`] returns
//! an error there, so jobs fail with `E_SANDBOX_UNAVAILABLE`. There is no
//! unsandboxed fallback.

use std::path::{Path, PathBuf};
use std::process::Command;

use converter_scheduler::{LaunchSpec, Launcher};
use serde::{Deserialize, Serialize};

#[cfg(windows)]
pub mod windows;

/// The shim's exit code when it can't apply the sandbox. The supervisor maps
/// it to `E_SANDBOX_UNAVAILABLE`.
pub use converter_scheduler::SANDBOX_SETUP_FAILED_EXIT;

/// Memory ceiling for requests made before admission (validate, estimate),
/// when [`LaunchSpec::memory_limit`] is `None`.
pub const DEFAULT_MEMORY_LIMIT: u64 = 1 << 30;

/// Name of the capability that grants read access to the worker runtime
/// (Python, its packages and the adapters). Only our workers hold it.
pub const RUNTIME_CAPABILITY: &str = "lfcsWorkerRuntime";

/// What the shim runs, and what the worker may touch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxSpec {
    /// Per-job container name. Jobs never share one, so one job can't read
    /// another job's directory.
    pub container: String,
    /// Absolute path of the program.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Directories the worker may read and write: only the job directory.
    pub read_write: Vec<PathBuf>,
    /// The worker's whole environment. Nothing is inherited.
    pub env: Vec<(String, String)>,
    pub memory_limit: u64,
    /// Most processes alive at once in the sandbox, the worker included.
    /// With 1, the worker also may not create child processes at all.
    pub process_limit: u32,
}

impl SandboxSpec {
    pub fn to_arg(&self) -> String {
        serde_json::to_string(self).expect("spec always serializes")
    }

    pub fn from_arg(arg: &str) -> Result<Self, String> {
        serde_json::from_str(arg).map_err(|e| format!("bad sandbox spec: {e}"))
    }
}

/// Container name for a job. AppContainer names allow at most 64
/// characters from letters, digits, `.` and `-`.
/// Prefix of every container name; profiles with it belong to us.
pub const PROFILE_PREFIX: &str = "lfcs.worker.";

pub fn container_name(job_id: &str) -> String {
    let id: String = job_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut name = format!("{PROFILE_PREFIX}{id}");
    name.truncate(64);
    name
}

/// Where the worker's Python and adapters live.
#[derive(Debug, Clone)]
pub struct WorkerRuntime {
    /// The interpreter itself, not a virtualenv launcher: a venv's
    /// `python.exe` on Windows starts the real one as a child process,
    /// which the sandbox forbids.
    pub python: PathBuf,
    /// Extra site directories (processing their `.pth` files), such as a
    /// development virtualenv's `site-packages`. Empty for a bundled
    /// runtime, whose packages sit in its own `site-packages`.
    pub site_dirs: Vec<PathBuf>,
    /// Holds `<adapter-id>/adapter.py`.
    pub adapters_dir: PathBuf,
}

impl WorkerRuntime {
    /// The development runtime: the workspace virtualenv made by `uv sync`.
    pub fn from_venv(venv: &Path, adapters_dir: &Path) -> Result<Self, String> {
        let cfg = std::fs::read_to_string(venv.join("pyvenv.cfg"))
            .map_err(|e| format!("no pyvenv.cfg in {}: {e}", venv.display()))?;
        let home = cfg
            .lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == "home")
            .map(|(_, v)| PathBuf::from(v.trim()))
            .ok_or("pyvenv.cfg has no home")?;
        let (python, site) = if cfg!(windows) {
            (
                home.join("python.exe"),
                venv.join("Lib").join("site-packages"),
            )
        } else {
            let version = cfg
                .lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == "version_info")
                .map(|(_, v)| v.trim().split('.').take(2).collect::<Vec<_>>().join("."))
                .ok_or("pyvenv.cfg has no version_info")?;
            (
                home.join("python3"),
                venv.join("lib")
                    .join(format!("python{version}"))
                    .join("site-packages"),
            )
        };
        let canon = |p: &Path| {
            std::fs::canonicalize(p)
                .map(strip_verbatim)
                .map_err(|e| format!("{}: {e}", p.display()))
        };
        Ok(WorkerRuntime {
            python: canon(&python)?,
            site_dirs: vec![canon(&site)?],
            adapters_dir: canon(adapters_dir)?,
        })
    }

    /// Directories the worker must be able to read: the interpreter's
    /// install, the site directories, the adapters, and whatever the site
    /// directories' path files point at (editable installs).
    pub fn read_only_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(home) = self.python.parent() {
            dirs.push(home.to_path_buf());
        }
        for site in &self.site_dirs {
            dirs.push(site.clone());
            dirs.extend(pth_targets(site));
        }
        dirs.push(self.adapters_dir.clone());
        dirs.sort();
        dirs.dedup();
        dirs
    }
}

/// Plain directory lines from `*.pth` files (lines starting with `import`
/// run code and are skipped).
fn pth_targets(site: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(site) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "pth") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with("import") {
                continue;
            }
            let p = site.join(line);
            if p.is_dir() {
                if let Ok(c) = std::fs::canonicalize(&p) {
                    out.push(strip_verbatim(c));
                }
            }
        }
    }
    out
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

/// Starts Python workers through the `sandbox-launch` shim.
#[derive(Debug, Clone)]
pub struct SandboxLauncher {
    shim: PathBuf,
    runtime: WorkerRuntime,
}

/// Runs the adapter with the site directories added. `-I` ignores
/// `PYTHONPATH`, so they're passed as arguments:
/// `-c BOOT <n> <site>... <script> <jobId>`.
///
/// It also pins temporary files to the job's `work/`: Windows rewrites
/// `TEMP` for AppContainer processes to the container's profile folder,
/// which job garbage collection doesn't cover.
const BOOTSTRAP: &str = "import os,runpy,site,sys,tempfile\n\
tempfile.tempdir=os.path.join(os.getcwd(),'work')\n\
n=int(sys.argv[1])\n\
for d in sys.argv[2:2+n]: site.addsitedir(d)\n\
s=sys.argv[2+n]\n\
sys.argv=[s]+sys.argv[3+n:]\n\
runpy.run_path(s,run_name='__main__')\n";

impl SandboxLauncher {
    /// Prepares the runtime for sandboxed use. On Windows this grants the
    /// runtime capability read access to [`WorkerRuntime::read_only_dirs`],
    /// once (an installed build gets this from the installer, spec 9.2).
    pub fn new(shim: PathBuf, runtime: WorkerRuntime) -> Result<Self, String> {
        if !shim.is_file() {
            return Err(format!("sandbox shim missing: {}", shim.display()));
        }
        #[cfg(windows)]
        windows::grant_runtime(&runtime.read_only_dirs())?;
        Ok(SandboxLauncher { shim, runtime })
    }

    /// The spec the shim gets for one launch.
    pub fn sandbox_spec(&self, spec: &LaunchSpec) -> Result<SandboxSpec, String> {
        let id = &spec.adapter_id;
        let valid = !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid {
            return Err(format!("bad adapter id {id:?}"));
        }
        let script = self.runtime.adapters_dir.join(id).join("adapter.py");
        let job_dir = std::path::absolute(&spec.job_dir).map_err(|e| e.to_string())?;
        let utf8 = |p: &Path| {
            p.to_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("path isn't UTF-8: {}", p.display()))
        };

        let mut args = vec![
            "-I".to_owned(),
            "-B".to_owned(),
            "-c".to_owned(),
            BOOTSTRAP.to_owned(),
            self.runtime.site_dirs.len().to_string(),
        ];
        for d in &self.runtime.site_dirs {
            args.push(utf8(d)?);
        }
        args.push(utf8(&script)?);
        args.push(spec.job_id.as_str().to_owned());

        let work = utf8(&job_dir.join("work"))?;
        let mut env = vec![
            ("TEMP".to_owned(), work.clone()),
            ("TMP".to_owned(), work.clone()),
            ("TMPDIR".to_owned(), work),
            ("PYTHONIOENCODING".to_owned(), "utf-8".to_owned()),
            ("PYTHONUTF8".to_owned(), "1".to_owned()),
        ];
        // Windows needs the first two to load system DLLs. CreateProcess
        // refuses to start an AppContainer process without LOCALAPPDATA
        // (error 203), and rewrites it to the container's own folder.
        for key in ["SystemRoot", "windir", "LOCALAPPDATA"] {
            if let Ok(v) = std::env::var(key) {
                env.push((key.to_owned(), v));
            }
        }

        Ok(SandboxSpec {
            container: container_name(spec.job_id.as_str()),
            program: self.runtime.python.clone(),
            args,
            cwd: job_dir.clone(),
            read_write: vec![job_dir],
            env,
            memory_limit: spec.memory_limit.unwrap_or(DEFAULT_MEMORY_LIMIT),
            // PDF workers run alone (spec 9.2); engines set their own later.
            process_limit: 1,
        })
    }
}

impl Launcher for SandboxLauncher {
    fn command(&self, spec: &LaunchSpec) -> Result<Command, String> {
        if !cfg!(windows) {
            // T10: bubblewrap on Linux, Seatbelt or App Sandbox on macOS.
            return Err("no sandbox for this OS yet".into());
        }
        let sandbox = self.sandbox_spec(spec)?;
        let mut cmd = Command::new(&self.shim);
        cmd.arg(sandbox.to_arg());
        // The shim itself gets a minimal environment too; it passes none of
        // it on.
        cmd.env_clear();
        for key in ["SystemRoot", "windir"] {
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        Ok(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_names_are_valid() {
        assert_eq!(container_name("j_01ab"), "lfcs.worker.j-01ab");
        let long = container_name(&"x".repeat(100));
        assert_eq!(long.len(), 64);
        assert!(long
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    }

    #[test]
    fn spec_round_trips() {
        let spec = SandboxSpec {
            container: "c".into(),
            program: "C:\\py\\python.exe".into(),
            args: vec!["a b".into(), "\"q\"".into()],
            cwd: "C:\\j".into(),
            read_write: vec!["C:\\j".into()],
            env: vec![("A".into(), "1".into())],
            memory_limit: 5,
            process_limit: 1,
        };
        assert_eq!(SandboxSpec::from_arg(&spec.to_arg()).unwrap(), spec);
        assert!(SandboxSpec::from_arg("{").is_err());
    }

    #[test]
    fn adapter_ids_cannot_escape() {
        let dir = std::env::temp_dir();
        let launcher = SandboxLauncher {
            shim: dir.join("shim"),
            runtime: WorkerRuntime {
                python: dir.join("python"),
                site_dirs: vec![],
                adapters_dir: dir.clone(),
            },
        };
        let spec = |adapter: &str| LaunchSpec {
            adapter_id: adapter.into(),
            job_id: converter_jobstore::JobId::parse("j_1").unwrap(),
            job_dir: dir.join("j_1"),
            memory_limit: None,
        };
        assert!(launcher.sandbox_spec(&spec("../evil")).is_err());
        assert!(launcher.sandbox_spec(&spec("a\\b")).is_err());
        assert!(launcher.sandbox_spec(&spec("")).is_err());
        let ok = launcher.sandbox_spec(&spec("pdf-merge")).unwrap();
        assert_eq!(ok.memory_limit, DEFAULT_MEMORY_LIMIT);
        assert_eq!(ok.read_write.len(), 1);
        assert!(ok.args.last().unwrap() == "j_1");
    }
}
