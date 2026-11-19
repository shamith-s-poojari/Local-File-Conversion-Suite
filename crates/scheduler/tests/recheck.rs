//! The re-check of a job's outputs in a separate check job (ADR 0006), with
//! real worker processes: `dev-fake` is the tool, and
//! `tests/workers/checker.py` stands in for `output-check`.
//!
//! Like `runner.rs`, they need the workspace virtualenv and run without a
//! sandbox (the product has none of that launcher).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use converter_jobstore::{
    DataRoot, InputSource, JobId, JobSnapshot, JobState, JobStore, StoreConfig, SyncRoots,
};
use converter_scheduler::{
    FixedMemory, JobSpec, LaunchSpec, Launcher, Recheck, Runner, RunnerConfig, Timing,
};
use converter_sniffer::Format;
use serde_json::json;
use tempfile::TempDir;

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
/// The same file with what the stand-in checker rejects in it.
const BAD_PDF: &[u8] = b"%PDF-1.7\n% BAD\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python() -> Option<PathBuf> {
    let venv = repo_root().join(".venv");
    [
        venv.join("Scripts").join("python.exe"),
        venv.join("bin").join("python"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

macro_rules! need_python {
    () => {
        match python() {
            Some(p) => p,
            None => {
                assert!(
                    std::env::var_os("CI").is_none(),
                    "no .venv under CI; the end-to-end tests must run"
                );
                eprintln!("skipped: no .venv; run `uv sync`");
                return;
            }
        }
    };
}

/// The tool is `dev-fake`; the check adapter is `checker.py` in `mode`.
/// Records the adapters it was asked to launch.
struct Launch {
    python: PathBuf,
    mode: &'static str,
    launched: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Launcher for Launch {
    fn command(&self, spec: &LaunchSpec) -> Result<Command, String> {
        self.launched.lock().unwrap().push(spec.adapter_id.clone());
        let mut cmd = Command::new(&self.python);
        cmd.arg("-I");
        if spec.adapter_id == "output-check" {
            cmd.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/workers/checker.py"))
                .arg(spec.job_id.as_str())
                .arg(&spec.adapter_id)
                .arg(self.mode);
        } else {
            cmd.arg(repo_root().join("adapters/dev-fake/adapter.py"))
                .arg(spec.job_id.as_str());
        }
        cmd.current_dir(&spec.job_dir);
        Ok(cmd)
    }
}

struct Rig {
    _tmp: TempDir,
    tmp: PathBuf,
    store: Arc<JobStore>,
    runner: Arc<Runner>,
    launched: Arc<std::sync::Mutex<Vec<String>>>,
}

fn rig(python: PathBuf, mode: &'static str) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let (store, _) = JobStore::open(root, SyncRoots::none(), StoreConfig::default()).unwrap();
    let store = Arc::new(store);
    let launched = Arc::default();
    let runner = Runner::new(
        store.clone(),
        Arc::new(Launch {
            python,
            mode,
            launched: Arc::clone(&launched),
        }),
        Arc::new(FixedMemory(64 << 30)),
        RunnerConfig {
            timing: Timing {
                hello: Duration::from_secs(20),
                cancel_grace: Duration::from_millis(300),
                exit_grace: Duration::from_millis(500),
            },
            ..RunnerConfig::default()
        },
    );
    Rig {
        tmp: tmp.path().to_path_buf(),
        _tmp: tmp,
        store,
        runner,
        launched,
    }
}

impl Rig {
    /// A staged job over a file holding `bytes`, whose part declares a
    /// re-check that fails with `E_REDACTION_VERIFICATION_FAILED`.
    fn spec(&self, bytes: &[u8], recheck: bool) -> JobSpec {
        let input = self.tmp.join("input.pdf");
        std::fs::write(&input, bytes).unwrap();
        let (id, _) = self.store.create_job().unwrap();
        self.store
            .stage_inputs(
                &id,
                &[InputSource {
                    path: input,
                    extension: "pdf".into(),
                }],
                1.0,
            )
            .unwrap();
        let mut spec = JobSpec::single(
            id,
            "dev-fake",
            "pdf",
            Format::Pdf,
            1,
            (30, 1.0, 10),
            json!({"pages": 1, "delayMs": 1}),
            None,
        );
        if recheck {
            spec.parts[0].recheck = Some(Recheck {
                adapter_id: "output-check".into(),
                timeout_base_s: 30,
                timeout_s_per_mb: 1.0,
                no_progress_s: 5,
                params: json!({"verifier": "test", "n": 1}),
                failure_code: "E_REDACTION_VERIFICATION_FAILED".into(),
            });
        }
        spec
    }

    fn run(&self, spec: JobSpec) -> JobSnapshot {
        let job = spec.job.clone();
        self.runner.start(spec).join().unwrap();
        self.job(&job)
    }

    fn job(&self, id: &JobId) -> JobSnapshot {
        self.store
            .snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == id)
            .unwrap()
    }

    /// The tool job's folder, with anything a check left in it.
    fn entries(&self, id: &JobId) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.store.job_dirs(id).unwrap().root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn outputs(&self, id: &JobId) -> Vec<String> {
        // A failed job's `out/` is deleted with its outputs.
        std::fs::read_dir(self.store.job_dirs(id).unwrap().out)
            .into_iter()
            .flatten()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}

fn error_code(job: &JobSnapshot) -> Option<&str> {
    job.error.as_ref().map(|e| e.code.as_str())
}

/// The core of S10: the tool's worker (dev-fake) reports success and writes
/// its output, and only the separate check decides whether it stands.
#[test]
fn a_passing_check_lets_the_job_succeed() {
    let r = rig(need_python!(), "verdict");
    let spec = r.spec(PDF, true);
    let id = spec.job.clone();
    let job = r.run(spec);
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    assert_eq!(job.outputs.len(), 1);
    assert_eq!(
        *r.launched.lock().unwrap().last().unwrap(),
        "output-check",
        "the check ran after the tool"
    );
    // The check job is discarded: its folder is gone, and it never reached
    // the store, so it can't be in a snapshot or an event.
    assert!(
        r.entries(&id).iter().all(|n| !n.starts_with("recheck-")),
        "{:?}",
        r.entries(&id)
    );
    assert_eq!(r.store.snapshot().jobs.len(), 1);
}

#[test]
fn a_failing_check_fails_the_job_even_though_its_worker_reported_success() {
    let r = rig(need_python!(), "verdict");
    let spec = r.spec(BAD_PDF, true);
    let id = spec.job.clone();
    let job = r.run(spec);
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    assert_eq!(error_code(&job), Some("E_REDACTION_VERIFICATION_FAILED"));
    assert!(job.outputs.is_empty());
    assert!(r.outputs(&id).is_empty(), "the output must be deleted");
    assert_eq!(r.store.snapshot().jobs.len(), 1);
}

/// The same output without a declared re-check: nothing is launched for it.
#[test]
fn a_job_without_a_recheck_runs_no_check() {
    let r = rig(need_python!(), "verdict");
    let job = r.run(r.spec(BAD_PDF, false));
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    assert!(!r.launched.lock().unwrap().contains(&"output-check".into()));
}

#[test]
fn a_check_that_crashes_is_a_failed_check_never_a_pass() {
    let r = rig(need_python!(), "crash");
    let spec = r.spec(PDF, true);
    let id = spec.job.clone();
    let job = r.run(spec);
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    assert_eq!(error_code(&job), Some("E_REDACTION_VERIFICATION_FAILED"));
    assert!(r.outputs(&id).is_empty());
}

#[test]
fn a_check_that_cannot_start_is_a_failed_check() {
    struct NoCheck(Launch);
    impl Launcher for NoCheck {
        fn command(&self, spec: &LaunchSpec) -> Result<Command, String> {
            if spec.adapter_id == "output-check" {
                return Err("no sandbox for the check".into());
            }
            self.0.command(spec)
        }
    }
    let py = need_python!();
    let mut r = rig(py.clone(), "verdict");
    r.runner = Runner::new(
        r.store.clone(),
        Arc::new(NoCheck(Launch {
            python: py,
            mode: "verdict",
            launched: Arc::default(),
        })),
        Arc::new(FixedMemory(64 << 30)),
        RunnerConfig::default(),
    );
    let job = r.run(r.spec(PDF, true));
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    assert_eq!(error_code(&job), Some("E_REDACTION_VERIFICATION_FAILED"));
}

#[test]
fn an_unreadable_or_unconvincing_report_is_a_failed_check() {
    let py = need_python!();
    for mode in ["garbage_report", "no_verdict", "failed_ids"] {
        let r = rig(py.clone(), mode);
        let job = r.run(r.spec(PDF, true));
        assert_eq!(job.state, JobState::Failed, "{mode}: {job:?}");
        assert!(job.outputs.is_empty(), "{mode}");
        assert!(
            matches!(
                error_code(&job),
                Some("E_REDACTION_VERIFICATION_FAILED" | "E_OUTPUT_INVALID")
            ),
            "{mode}: {job:?}"
        );
    }
}

#[test]
fn a_check_that_runs_too_long_is_a_failed_check() {
    let r = rig(need_python!(), "slow");
    let mut spec = r.spec(PDF, true);
    spec.parts[0].recheck.as_mut().unwrap().timeout_base_s = 2;
    let started = Instant::now();
    let job = r.run(spec);
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    assert_eq!(error_code(&job), Some("E_REDACTION_VERIFICATION_FAILED"));
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn cancelling_the_job_during_the_check_cancels_the_check() {
    let r = rig(need_python!(), "slow");
    let spec = r.spec(PDF, true);
    let id = spec.job.clone();
    let handle = r.runner.start(spec);
    // The check is running once its worker has left its marker.
    let deadline = Instant::now() + Duration::from_secs(30);
    let marker = loop {
        let found = std::fs::read_dir(r.store.job_dirs(&id).unwrap().root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("work/started").exists());
        if let Some(p) = found {
            break p;
        }
        assert!(Instant::now() < deadline, "the check never started");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(r.job(&id).state, JobState::Verifying);
    let started = Instant::now();
    assert!(r.runner.cancel(&id));
    handle.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "the check outlived the cancel"
    );
    assert_eq!(r.job(&id).state, JobState::Cancelled);
    assert!(!marker.exists(), "the check job's folder is discarded");
}

/// What the check gets is what the core said, and nothing of the tool job.
/// (The stand-in fails the report unless `in/` holds exactly `0.pdf` and the
/// params are the core's; `a_passing_check_lets_the_job_succeed` is that.)
#[test]
fn the_check_params_come_from_the_core_not_the_worker() {
    let r = rig(need_python!(), "verdict");
    let mut spec = r.spec(PDF, true);
    spec.parts[0].recheck.as_mut().unwrap().params = json!({"verifier": "other"});
    let job = r.run(spec);
    assert_eq!(job.state, JobState::Failed, "{job:?}");
}
