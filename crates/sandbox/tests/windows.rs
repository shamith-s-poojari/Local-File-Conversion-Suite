//! Windows sandbox tests: the real `sandbox-launch` shim, AppContainer and
//! Job Object, with the workspace's Python (`uv sync`). Without the
//! virtualenv they print a note and pass, like the scheduler's tests.
//!
//! The first run grants the runtime capability read access to the Python
//! install, the virtualenv's `site-packages`, `workers/sdk/src` and
//! `adapters/` (spec 9.2; an installer does this for a release build).
#![cfg(windows)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use converter_jobstore::{DataRoot, InputSource, JobState, JobStore, StoreConfig, SyncRoots};
use converter_sandbox::{
    container_name, SandboxLauncher, SandboxSpec, WorkerRuntime, SANDBOX_SETUP_FAILED_EXIT,
};
use converter_scheduler::{FixedMemory, JobSpec, Runner, RunnerConfig};
use converter_sniffer::Format;
use serde_json::{json, Value};

const SHIM: &str = env!("CARGO_BIN_EXE_sandbox-launch");

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn runtime() -> Option<WorkerRuntime> {
    let root = repo_root();
    let venv = root.join(".venv");
    if !venv.join("pyvenv.cfg").is_file() {
        eprintln!("skipped: no .venv; run `uv sync`");
        return None;
    }
    Some(WorkerRuntime::from_venv(&venv, &root.join("adapters")).unwrap())
}

macro_rules! need_runtime {
    () => {
        match runtime() {
            Some(r) => {
                converter_sandbox::windows::grant_runtime(&r.read_only_dirs()).unwrap();
                r
            }
            None => return,
        }
    };
}

struct Dirs {
    _tmp: tempfile::TempDir,
    job: PathBuf,
    other_job: PathBuf,
    outside: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let job = tmp.path().join("jobs").join("j_a");
    let other_job = tmp.path().join("jobs").join("j_b");
    let outside = tmp.path().join("outside");
    for d in [job.join("work"), other_job.clone(), outside.clone()] {
        std::fs::create_dir_all(&d).unwrap();
    }
    std::fs::write(job.join("in.txt"), "mine").unwrap();
    std::fs::write(other_job.join("secret.txt"), "theirs").unwrap();
    std::fs::write(outside.join("sentinel.txt"), "private").unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/probe.py"),
        job.join("probe.py"),
    )
    .unwrap();
    Dirs {
        _tmp: tmp,
        job,
        other_job,
        outside,
    }
}

fn probe_spec(rt: &WorkerRuntime, d: &Dirs, job_id: &str, mode: &str, args: Value) -> SandboxSpec {
    let s = |p: &Path| p.to_str().unwrap().to_owned();
    SandboxSpec {
        container: container_name(job_id),
        program: rt.python.clone(),
        args: vec![
            "-I".into(),
            "-B".into(),
            s(&d.job.join("probe.py")),
            mode.into(),
            args.to_string(),
        ],
        cwd: d.job.clone(),
        read_write: vec![d.job.clone()],
        env: vec![
            ("SystemRoot".into(), std::env::var("SystemRoot").unwrap()),
            (
                "LOCALAPPDATA".into(),
                std::env::var("LOCALAPPDATA").unwrap(),
            ),
            ("TEMP".into(), s(&d.job.join("work"))),
            ("TMP".into(), s(&d.job.join("work"))),
        ],
        memory_limit: 256 << 20,
        process_limit: 1,
    }
}

fn shim(spec: &SandboxSpec) -> Command {
    let mut cmd = Command::new(SHIM);
    cmd.arg(spec.to_arg())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn run_probe(spec: &SandboxSpec) -> Value {
    let out = shim(spec).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "probe failed: {:?}\n{stderr}",
        out.status
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("bad probe output {e}: {:?}\n{stderr}", out.stdout))
}

fn allowed(v: &Value) -> bool {
    v == "allowed"
}

#[test]
fn worker_reaches_only_its_job_directory() {
    let rt = need_runtime!();
    let d = dirs();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let args = json!({
        "own_file": d.job.join("in.txt"),
        "work": d.job.join("work"),
        "outside_file": d.outside.join("sentinel.txt"),
        "outside_dir": d.outside,
        "other_job_file": d.other_job.join("secret.txt"),
        "port": listener.local_addr().unwrap().port(),
    });
    let r = run_probe(&probe_spec(&rt, &d, "j_a", "access", args));

    // Controls: the probe isn't failing for the wrong reason.
    assert!(allowed(&r["read_own"]), "{r}");
    assert!(allowed(&r["write_work"]), "{r}");
    assert!(d.job.join("work/probe.txt").is_file());

    for denied in [
        "read_outside",
        "list_outside",
        "read_other_job",
        "write_outside",
        "loopback",
        "dns",
        "spawn",
    ] {
        assert!(!allowed(&r[denied]), "{denied} should be denied: {r}");
    }
    assert!(!d.outside.join("w.txt").exists());

    // Nothing from our environment leaks in.
    let env: Vec<String> = serde_json::from_value(r["env"].clone()).unwrap();
    let env: Vec<String> = env.into_iter().map(|k| k.to_uppercase()).collect();
    assert!(!env.contains(&"USERPROFILE".to_owned()), "{env:?}");
    assert!(!env.contains(&"PATH".to_owned()), "{env:?}");
}

#[test]
fn memory_limit_is_enforced() {
    let rt = need_runtime!();
    let d = dirs();
    let r = run_probe(&probe_spec(&rt, &d, "j_mem", "memory", json!({"mb": 600})));
    assert_eq!(r["allocate"], "denied:MemoryError", "{r}");

    let r = run_probe(&probe_spec(&rt, &d, "j_mem", "memory", json!({"mb": 50})));
    assert!(allowed(&r["allocate"]), "{r}");
}

#[test]
fn killing_the_shim_kills_the_worker() {
    let rt = need_runtime!();
    let d = dirs();
    let spec = probe_spec(
        &rt,
        &d,
        "j_kill",
        "sleep",
        json!({"work": d.job.join("work")}),
    );
    let mut child = shim(&spec).spawn().unwrap();
    let pid_file = d.job.join("work/pid");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !pid_file.is_file() || std::fs::read_to_string(&pid_file).unwrap().is_empty() {
        assert!(Instant::now() < deadline, "worker never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));

    child.kill().unwrap();
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while alive(pid) {
        assert!(Instant::now() < deadline, "worker {pid} outlived the shim");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A killed shim can't delete its profile; the startup sweep does.
    // (Scoped to this test's container: other tests run in parallel.)
    use converter_sandbox::windows::remove_stale_profiles;
    assert_eq!(remove_stale_profiles(&spec.container), 1);
    assert_eq!(remove_stale_profiles(&spec.container), 0);
}

fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn setup_failure_has_its_own_exit_code() {
    let mut spec = SandboxSpec {
        container: "lfcs.worker.bad".into(),
        program: "python.exe".into(), // not absolute
        args: vec![],
        cwd: std::env::temp_dir(),
        read_write: vec![],
        env: vec![],
        memory_limit: 1 << 30,
        process_limit: 1,
    };
    let code = shim(&spec).status().unwrap().code();
    assert_eq!(code, Some(SANDBOX_SETUP_FAILED_EXIT));

    spec.program = std::env::temp_dir().join("does-not-exist.exe");
    let code = shim(&spec).status().unwrap().code();
    assert_eq!(code, Some(SANDBOX_SETUP_FAILED_EXIT));

    let code = Command::new(SHIM).arg("{").status().unwrap().code();
    assert_eq!(code, Some(SANDBOX_SETUP_FAILED_EXIT));
}

fn job_spec(job: &converter_jobstore::JobId) -> JobSpec {
    JobSpec {
        job: job.clone(),
        adapter_id: "dev-fake".into(),
        pool: "pdf".into(),
        output_format: Format::Pdf,
        timeout_base_s: 60,
        timeout_s_per_mb: 1.0,
        no_progress_s: 30,
        params: json!({}),
        secrets: None,
    }
}

fn run_job(launcher: SandboxLauncher) -> converter_jobstore::JobSnapshot {
    let tmp = tempfile::tempdir().unwrap();
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let (store, _) = JobStore::open(root, SyncRoots::none(), StoreConfig::default()).unwrap();
    let store = Arc::new(store);
    let input = tmp.path().join("input.pdf");
    std::fs::write(&input, PDF).unwrap();
    let (id, _) = store.create_job().unwrap();
    store
        .stage_inputs(
            &id,
            &[InputSource {
                path: input,
                extension: "pdf".into(),
            }],
            1.0,
        )
        .unwrap();
    let runner = Runner::new(
        store.clone(),
        Arc::new(launcher),
        Arc::new(FixedMemory(8 << 30)),
        RunnerConfig::default(),
    );
    runner.start(job_spec(&id)).join().unwrap();
    store
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.job_id == id)
        .unwrap()
}

#[test]
fn dev_fake_runs_end_to_end_in_the_sandbox() {
    let rt = need_runtime!();
    let launcher = SandboxLauncher::new(PathBuf::from(SHIM), rt).unwrap();
    let job = run_job(launcher);
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
}

#[test]
fn broken_runtime_fails_as_sandbox_unavailable() {
    let Some(mut rt) = runtime() else { return };
    let tmp = tempfile::tempdir().unwrap();
    rt.python = tmp.path().join("python.exe");
    rt.site_dirs.clear();
    let launcher = SandboxLauncher::new(PathBuf::from(SHIM), rt).unwrap();
    let job = run_job(launcher);
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    assert_eq!(
        job.error.as_ref().map(|e| e.code.as_str()),
        Some("E_SANDBOX_UNAVAILABLE"),
        "{job:?}"
    );
}
