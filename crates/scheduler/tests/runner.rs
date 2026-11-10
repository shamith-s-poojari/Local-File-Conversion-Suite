//! End-to-end supervisor tests with real worker processes: the `dev-fake`
//! adapter through the Python SDK, and `tests/workers/misbehave.py`.
//!
//! They need the workspace virtualenv (`uv sync`). Without it they print a
//! note and pass, so `cargo test` still works on a fresh checkout; CI always
//! runs `uv sync` first.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use converter_jobstore::{
    DataRoot, InputSource, JobId, JobSnapshot, JobState, JobStore, StoreConfig, SyncRoots,
};
use converter_scheduler::{
    FixedMemory, JobSpec, LaunchSpec, Launcher, Runner, RunnerConfig, Timing, GENERIC_WARNING_CODE,
    MAX_DETAIL_BYTES,
};
use converter_sniffer::Format;
use serde_json::{json, Value};
use tempfile::TempDir;

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";

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

/// Runs workers straight from the repo, **without a sandbox**. Tests only:
/// the product has no unsandboxed launcher.
struct TestLauncher {
    python: PathBuf,
    /// Set for `misbehave.py`; `None` runs the dev-fake adapter.
    mode: Option<&'static str>,
}

impl Launcher for TestLauncher {
    fn command(&self, spec: &LaunchSpec) -> Result<Command, String> {
        let mut cmd = Command::new(&self.python);
        cmd.arg("-I");
        match self.mode {
            None => {
                cmd.arg(repo_root().join("adapters/dev-fake/adapter.py"))
                    .arg(spec.job_id.as_str());
            }
            Some(mode) => {
                cmd.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/workers/misbehave.py"))
                    .arg(spec.job_id.as_str())
                    .arg(&spec.adapter_id)
                    .arg(mode);
            }
        }
        cmd.current_dir(&spec.job_dir);
        Ok(cmd)
    }
}

struct NoSandbox;

impl Launcher for NoSandbox {
    fn command(&self, _: &LaunchSpec) -> Result<Command, String> {
        Err("sandbox unavailable".into())
    }
}

struct Env {
    _tmp: TempDir,
    input: PathBuf,
    store: Arc<JobStore>,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let (store, _) = JobStore::open(root, SyncRoots::none(), StoreConfig::default()).unwrap();
    let input = tmp.path().join("input.pdf");
    std::fs::write(&input, PDF).unwrap();
    Env {
        _tmp: tmp,
        input,
        store: Arc::new(store),
    }
}

fn config() -> RunnerConfig {
    RunnerConfig {
        timing: Timing {
            hello: Duration::from_secs(20),
            cancel_grace: Duration::from_millis(300),
            exit_grace: Duration::from_millis(500),
        },
        ..RunnerConfig::default()
    }
}

fn spec(job: &JobId, adapter_id: &str, params: Value) -> JobSpec {
    JobSpec {
        job: job.clone(),
        adapter_id: adapter_id.into(),
        pool: "pdf".into(),
        output_format: Format::Pdf,
        timeout_base_s: 30,
        timeout_s_per_mb: 1.0,
        no_progress_s: 10,
        params,
        secrets: None,
    }
}

impl Env {
    fn staged_job(&self) -> JobId {
        let (id, _) = self.store.create_job().unwrap();
        self.store
            .stage_inputs(
                &id,
                &[InputSource {
                    path: self.input.clone(),
                    extension: "pdf".into(),
                }],
                1.0,
            )
            .unwrap();
        id
    }

    fn job(&self, id: &JobId) -> JobSnapshot {
        self.store
            .snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == id)
            .unwrap()
    }

    fn run(
        &self,
        launcher: impl Launcher + 'static,
        cfg: RunnerConfig,
        spec: JobSpec,
    ) -> JobSnapshot {
        let runner = Runner::new(
            self.store.clone(),
            Arc::new(launcher),
            Arc::new(FixedMemory(64 << 30)),
            cfg,
        );
        let job = spec.job.clone();
        runner.start(spec).join().unwrap();
        self.job(&job)
    }
}

macro_rules! need_python {
    () => {
        match python() {
            Some(p) => p,
            None => {
                eprintln!("skipped: no .venv; run `uv sync`");
                return;
            }
        }
    };
}

fn fake(python: &Path) -> TestLauncher {
    TestLauncher {
        python: python.to_path_buf(),
        mode: None,
    }
}

fn bad(python: &Path, mode: &'static str) -> TestLauncher {
    TestLauncher {
        python: python.to_path_buf(),
        mode: Some(mode),
    }
}

fn error_code(job: &JobSnapshot) -> Option<&str> {
    job.error.as_ref().map(|e| e.code.as_str())
}

#[test]
fn dev_fake_runs_to_success_with_stray_stdout() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let job = e.run(
        fake(&py),
        config(),
        spec(
            &id,
            "dev-fake",
            json!({"pages": 3, "delayMs": 1, "noise": true, "warn": "TAGS_DROPPED"}),
        ),
    );
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    assert_eq!(job.outputs.len(), 1);
    assert!(job.warnings.iter().any(|w| w.code == "TAGS_DROPPED"));
    let dirs = e.store.job_dirs(&id).unwrap();
    assert_eq!(
        std::fs::read(dirs.out.join(&job.outputs[0].path)).unwrap(),
        PDF
    );
    assert!(!dirs.work.exists());
}

#[test]
fn adapter_errors_and_crashes_fail_the_job() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let job = e.run(
        fake(&py),
        config(),
        spec(
            &id,
            "dev-fake",
            json!({"failWith": "E_INPUT_CORRUPT", "delayMs": 1}),
        ),
    );
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(error_code(&job), Some("E_INPUT_CORRUPT"));
    assert!(!e.store.job_dirs(&id).unwrap().out.exists());

    let id = e.staged_job();
    let job = e.run(
        fake(&py),
        config(),
        spec(&id, "dev-fake", json!({"crash": true, "delayMs": 1})),
    );
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(error_code(&job), Some("E_INTERNAL"));
}

#[test]
fn watchdog_kills_a_hung_worker() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let mut s = spec(&id, "dev-fake", json!({"hang": true}));
    s.no_progress_s = 1;
    let started = std::time::Instant::now();
    let job = e.run(fake(&py), config(), s);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(error_code(&job), Some("E_TIMEOUT"));
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn cancel_stops_a_running_job() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let runner = Runner::new(
        e.store.clone(),
        Arc::new(fake(&py)),
        Arc::new(FixedMemory(64 << 30)),
        config(),
    );
    let handle = runner.start(spec(&id, "dev-fake", json!({"pages": 1000, "delayMs": 50})));
    // Wait until it's executing.
    for _ in 0..400 {
        if e.job(&id).state == JobState::Running {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(runner.cancel(&id));
    handle.join().unwrap();
    let job = e.job(&id);
    assert_eq!(job.state, JobState::Cancelled);
    assert!(!e.store.job_dirs(&id).unwrap().input_dir.exists());
}

#[test]
fn large_job_warns_and_runs_alone() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    // A 600-byte user setting caps the budget (the 60% term has a 512 MB
    // floor), so any estimate exceeds it.
    let mut cfg = config();
    cfg.user_memory_limit = Some(600);
    let runner = Runner::new(
        e.store.clone(),
        Arc::new(fake(&py)),
        Arc::new(FixedMemory(1000)),
        cfg,
    );
    runner
        .start(spec(&id, "dev-fake", json!({"pages": 1, "delayMs": 1})))
        .join()
        .unwrap();
    let job = e.job(&id);
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    assert!(job.warnings.iter().any(|w| w.code == "W_LARGE_JOB"));
}

#[test]
fn jobs_share_a_one_slot_pool() {
    let py = need_python!();
    let e = env();
    let mut cfg = config();
    cfg.pools.insert("pdf".into(), 1);
    let runner = Runner::new(
        e.store.clone(),
        Arc::new(fake(&py)),
        Arc::new(FixedMemory(64 << 30)),
        cfg,
    );
    let ids: Vec<JobId> = (0..3).map(|_| e.staged_job()).collect();
    let handles: Vec<_> = ids
        .iter()
        .map(|id| runner.start(spec(id, "dev-fake", json!({"pages": 2, "delayMs": 5}))))
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    for id in &ids {
        assert_eq!(e.job(id).state, JobState::Succeeded);
    }
}

#[test]
fn sandbox_failure_fails_closed() {
    let e = env();
    let id = e.staged_job();
    let job = e.run(NoSandbox, config(), spec(&id, "dev-fake", json!({})));
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(error_code(&job), Some("E_SANDBOX_UNAVAILABLE"));
}

#[test]
fn protocol_violations_kill_the_worker() {
    let py = need_python!();
    let cases = [
        ("bad_version", "E_INSTALL_CORRUPT"),
        ("wrong_job", "E_PROTOCOL"),
        ("garbage", "E_PROTOCOL"),
        ("long_line", "E_PROTOCOL"),
        ("flood", "E_PROTOCOL"),
        ("wrong_result", "E_PROTOCOL"),
        ("bad_hash", "E_PROTOCOL"),
        ("bad_size", "E_PROTOCOL"),
        ("wrong_format", "E_OUTPUT_INVALID"),
        ("exit_silently", "E_INTERNAL"),
        ("memory_always", "E_RESOURCE_MEMORY"),
    ];
    let e = env();
    for (mode, code) in cases {
        let id = e.staged_job();
        let job = e.run(bad(&py, mode), config(), spec(&id, "misbehave", json!({})));
        assert_eq!(job.state, JobState::Failed, "{mode}");
        assert_eq!(error_code(&job), Some(code), "{mode}");
        assert!(job.outputs.is_empty(), "{mode}");
    }
}

#[test]
fn missing_hello_times_out() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let mut cfg = config();
    cfg.timing.hello = Duration::from_secs(2);
    let job = e.run(bad(&py, "no_hello"), cfg, spec(&id, "misbehave", json!({})));
    assert_eq!(error_code(&job), Some("E_TIMEOUT"));
}

#[test]
fn hello_from_the_wrong_adapter_is_refused() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let launcher = bad(&py, "ok");
    // The launcher passes the real adapter id; claim a different one.
    struct Lie(TestLauncher);
    impl Launcher for Lie {
        fn command(&self, spec: &LaunchSpec) -> Result<Command, String> {
            let mut s = spec.clone();
            s.adapter_id = "someone-else".into();
            self.0.command(&s)
        }
    }
    let job = e.run(Lie(launcher), config(), spec(&id, "misbehave", json!({})));
    assert_eq!(error_code(&job), Some("E_INSTALL_CORRUPT"));
}

#[test]
fn unknown_warnings_become_generic_and_detail_is_capped() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let job = e.run(
        bad(&py, "unknown_warning"),
        config(),
        spec(&id, "misbehave", json!({})),
    );
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    let w = &job.warnings[0];
    assert_eq!(w.code, GENERIC_WARNING_CODE);
    assert!(w.detail.as_ref().unwrap().len() <= MAX_DETAIL_BYTES);
}

#[test]
fn memory_failure_retries_once_alone() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let job = e.run(
        bad(&py, "memory_once"),
        config(),
        spec(&id, "misbehave", json!({})),
    );
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
}

#[test]
fn killing_takes_the_whole_process_tree() {
    let py = need_python!();
    let e = env();
    let id = e.staged_job();
    let root = e.store.job_dirs(&id).unwrap().root;
    let mut s = spec(&id, "misbehave", json!({}));
    s.no_progress_s = 2;
    let job = e.run(bad(&py, "grandchild_hang"), config(), s);
    assert_eq!(error_code(&job), Some("E_TIMEOUT"));

    let pid: u32 = std::fs::read_to_string(root.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut alive = true;
    for _ in 0..50 {
        let mut sys = sysinfo::System::new();
        let p = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
        alive = sys.process(p).is_some_and(|p| {
            !matches!(
                p.status(),
                sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
            )
        });
        if !alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive, "the worker's child survived");
}
