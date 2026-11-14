//! The sandbox on this machine: the spec 9.3 probe, and a real job through
//! the scheduler with the sandbox launcher (the week 3 walking skeleton,
//! minus the UI).
//!
//! Needs the workspace virtualenv (`uv sync`). On Linux it also needs
//! bubblewrap able to create namespaces; without either it prints a note
//! and passes, like the scheduler's tests. On macOS and Windows the sandbox
//! must work.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use converter_jobstore::{
    DataRoot, InputSource, JobId, JobState, JobStore, StoreConfig, SyncRoots,
};
use converter_sandbox::{probe, SandboxConfig, SandboxLauncher};
use converter_scheduler::{
    FixedMemory, JobSpec, LaunchSpec, Launcher, Runner, RunnerConfig, Timing,
};
use converter_sniffer::Format;
use serde_json::json;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn launcher() -> Option<SandboxLauncher> {
    let python = if cfg!(windows) {
        repo_root().join(".venv/Scripts/python.exe")
    } else {
        repo_root().join(".venv/bin/python")
    };
    if !python.is_file() {
        eprintln!("skipped: no .venv; run `uv sync`");
        return None;
    }
    let config = SandboxConfig::discover(
        &python,
        &repo_root().join("adapters"),
        Path::new(env!("CARGO_BIN_EXE_converter-sandbox-run")),
    )
    .expect("runtime discovery");
    Some(SandboxLauncher::new(config))
}

fn available(l: &SandboxLauncher, scratch: &Path) -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        return true; // Must work here: no skipping.
    }
    // Linux: bubblewrap may be blocked (C7); a failed probe *report* still
    // counts, but not being able to start at all means "unavailable".
    let job = scratch.join("check");
    std::fs::create_dir_all(job.join("work")).unwrap();
    let argv = vec![
        l.config().python.clone().into_os_string(),
        "-I".into(),
        "-c".into(),
        "pass".into(),
    ];
    let mut cmd = match l.wrap("j_check", &job, 256 << 20, argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {e}");
            return false;
        }
    };
    let ok = cmd.status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: bubblewrap can't create a sandbox here");
    }
    ok
}

#[test]
fn probe_passes() {
    let Some(l) = launcher() else { return };
    let tmp = tempfile::tempdir().unwrap();
    if !available(&l, tmp.path()) {
        return;
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap();
    let report = probe::self_test(&l, tmp.path(), &home).unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    let failed: Vec<_> = report.attempts.iter().filter(|a| !a.passed).collect();
    assert!(report.passed, "probe failures: {failed:#?}");
    assert_eq!(report.attempts.len(), 12);
}

#[test]
fn a_job_runs_end_to_end_inside_the_sandbox() {
    let Some(l) = launcher() else { return };
    let tmp = tempfile::tempdir().unwrap();
    if !available(&l, tmp.path()) {
        return;
    }
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let (store, _) = JobStore::open(root, SyncRoots::none(), StoreConfig::default()).unwrap();
    let store = Arc::new(store);
    let input = tmp.path().join("input.pdf");
    std::fs::write(
        &input,
        b"%PDF-1.7\n1 0 obj << /Type /Page >> endobj\n2 0 obj << /Type /Page >> endobj\n%%EOF\n",
    )
    .unwrap();
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
        Arc::new(l),
        Arc::new(FixedMemory(64 << 30)),
        RunnerConfig {
            timing: Timing {
                hello: Duration::from_secs(30),
                ..Timing::default()
            },
            ..RunnerConfig::default()
        },
    );
    let spec = JobSpec::single(
        id.clone(),
        "dev-fake",
        "pdf",
        Format::Pdf,
        1,
        (60, 1.0, 30),
        json!({"pages": 3, "delayMs": 1}),
        None,
    );
    runner.start(spec).join().unwrap();
    let job = store
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.job_id == id)
        .unwrap();
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
}

/// Each adapter's worker starts in the sandbox and says hello. On failure
/// the worker's stderr is shown, which the app keeps to itself (it can
/// hold document text) but a test with no documents can print.
#[test]
fn real_adapters_start_inside_the_sandbox() {
    let Some(l) = launcher() else { return };
    let tmp = tempfile::tempdir().unwrap();
    if !available(&l, tmp.path()) {
        return;
    }
    l.prepare().expect("runtime grants");
    for adapter in ["dev-fake", "pdf-redact", "image-convert", "data-convert"] {
        let job = tmp.path().join(adapter);
        for d in ["in", "out", "work"] {
            std::fs::create_dir_all(job.join(d)).unwrap();
        }
        let spec = LaunchSpec {
            adapter_id: adapter.into(),
            job_id: JobId::parse("j_hello").unwrap(),
            job_dir: job,
            memory_limit: Some(1 << 30),
        };
        let mut cmd = l.command(&spec).expect("worker command");
        // No requests: the worker says hello, sees end of input and exits.
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = cmd.output().expect("the runner starts");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(r#""type":"hello""#),
            "{adapter} didn't say hello (exit {:?})\nstdout: {stdout}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
