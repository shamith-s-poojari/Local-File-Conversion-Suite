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

/// CI must run these tests, not skip them: a missing virtualenv or
/// bubblewrap there is a broken job, not a reason to pass.
fn skip(why: &str) {
    assert!(
        std::env::var_os("CI").is_none(),
        "not skipping on CI: {why}"
    );
    eprintln!("skipped: {why}");
}

/// The checkout's own `.venv`, found from this crate's location; neither
/// `LFCS_DEV` nor the working directory is involved.
fn launcher() -> Option<SandboxLauncher> {
    let python = if cfg!(windows) {
        repo_root().join(".venv/Scripts/python.exe")
    } else {
        repo_root().join(".venv/bin/python")
    };
    if !python.is_file() {
        skip("no .venv; run `uv sync`");
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
            skip(&e);
            return false;
        }
    };
    let ok = cmd.status().is_ok_and(|s| s.success());
    if !ok {
        skip("bubblewrap can't create a sandbox here");
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
    // 8 refusals every platform has, a hard link, and (Unix) a symlink and a
    // Unix socket; 2 allowed; nothing written outside; the memory limit; and
    // on Linux /proc/1/environ.
    // Linux adds the seccomp ptrace check (x86-64 and AArch64). This virtualenv
    // grants no engine, so there are no engine checks.
    let seccomp = usize::from(cfg!(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )));
    let expected = 8
        + 1
        + 2 * usize::from(cfg!(unix))
        + 2
        + 1
        + 1
        + usize::from(cfg!(target_os = "linux"))
        + seccomp;
    assert_eq!(report.attempts.len(), expected);
    // Every refusal must be the sandbox's own error, not an accident.
    for a in &report.attempts {
        assert_ne!(a.outcome, "inconclusive", "{a:?}");
    }
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

/// "Blocked" must mean the expected errno: a missing path or any other
/// failure is `inconclusive`, never evidence that the sandbox works.
#[test]
fn the_probe_counts_only_the_expected_errno_as_blocked() {
    let Some(l) = launcher() else { return };
    let probe_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        probe_dir.path().join("probe.py"),
        include_str!("../probe/probe.py"),
    )
    .unwrap();
    // EPERM is 1 and ENOENT 2 everywhere we run; the file can't exist.
    let script = format!(
        "import sys, json; sys.path.insert(0, {dir:?}); import probe\n\
         missing = probe.attempt(lambda: open({gone:?}), {{1}})\n\
         refused = probe.attempt(lambda: open({gone:?}), {{2}})\n\
         ok = probe.attempt(lambda: None, {{1}})\n\
         print(json.dumps([missing['outcome'], refused['outcome'], ok['outcome']]))",
        dir = probe_dir.path().to_string_lossy(),
        gone = probe_dir.path().join("gone/x").to_string_lossy(),
    );
    let out = std::process::Command::new(&l.config().python)
        .args(["-I", "-c", &script])
        .output()
        .unwrap();
    let got: Vec<String> =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{:?}", out));
    assert_eq!(got, ["inconclusive", "blocked", "allowed"]);
}

/// The Unix-socket check needs a path under ~104 bytes (`sun_path`); a deep
/// TMPDIR must not make the self-test fail, or processing would be switched
/// off for real users.
#[cfg(unix)]
#[test]
fn probe_passes_with_a_long_scratch_path() {
    let Some(l) = launcher() else { return };
    let tmp = tempfile::tempdir().unwrap();
    if !available(&l, tmp.path()) {
        return;
    }
    let deep = tmp.path().join("d".repeat(60)).join("e".repeat(60));
    std::fs::create_dir_all(&deep).unwrap();
    assert!(deep.to_string_lossy().len() > 110);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let report = probe::self_test(&l, &deep, &home).unwrap();
    let failed: Vec<_> = report.attempts.iter().filter(|a| !a.passed).collect();
    assert!(report.passed, "probe failures: {failed:#?}");
    assert!(report.attempts.iter().any(|a| a.name == "unix_socket"));
}

/// SEC-9: an engine's granted folder must read but not write from inside.
#[test]
fn the_probe_checks_what_engines_are_granted() {
    let Some(mut l) = launcher() else { return };
    let tmp = tempfile::tempdir().unwrap();
    if !available(&l, tmp.path()) {
        return;
    }
    let engine = tempfile::tempdir().unwrap();
    std::fs::write(engine.path().join("bin"), b"engine").unwrap();
    let mut config = l.config().clone();
    config.grants.insert(
        "x-engine".into(),
        converter_sandbox::Grant {
            read: vec![engine.path().canonicalize().unwrap()],
            ..Default::default()
        },
    );
    l = SandboxLauncher::new(config);
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap();
    let report = probe::self_test(&l, tmp.path(), &home).unwrap();
    let engine_checks: Vec<_> = report
        .attempts
        .iter()
        .filter(|a| a.name.starts_with("engine_"))
        .collect();
    assert_eq!(engine_checks.len(), 2, "{engine_checks:#?}");
    assert!(engine_checks.iter().all(|a| a.passed), "{engine_checks:#?}");
    assert!(report.passed, "{report:#?}");
    assert!(
        !engine.path().join(".lfcs-probe").exists(),
        "the probe left a file in the engine folder"
    );
}
