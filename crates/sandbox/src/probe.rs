//! Sandbox self-test (spec section 9.3). Runs at first launch and after every
//! update; if it fails, the app shows an error and disables processing.
//!
//! The probe tries what the sandbox must refuse and what it must allow, in
//! a scratch job directory, then allocates past a small limit to prove the
//! runner kills it and reports `E_RESOURCE_MEMORY`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use serde_json::Value;

use crate::SandboxLauncher;

/// Embedded so the probe can't go missing or be swapped on disk.
const PROBE: &str = include_str!("../probe/probe.py");
const PROBE_JOB_ID: &str = "j_probe";
const ACCESS_LIMIT: u64 = 1024 * 1024 * 1024;
const ALLOC_LIMIT: u64 = 256 * 1024 * 1024;

/// Must be refused.
const DENIED: &[&str] = &[
    "read_sentinel",
    "list_home",
    "read_other_job",
    "write_outside",
    "connect_external",
    "connect_loopback",
    "dns",
    "spawn_shell",
];
/// Must work, or the probe proves nothing.
const ALLOWED: &[&str] = &["read_job", "write_work"];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Attempt {
    pub name: String,
    pub expected: &'static str,
    pub outcome: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub attempts: Vec<Attempt>,
    pub passed: bool,
}

/// Runs the probe in `scratch` (created if needed, under app data, never a
/// synced folder). `home` is the user's home directory.
pub fn self_test(
    launcher: &SandboxLauncher,
    scratch: &Path,
    home: &Path,
) -> Result<ProbeReport, String> {
    let root = scratch.join("probe");
    let _ = fs::remove_dir_all(&root);
    let job = root.join("job-a");
    let other = root.join("job-b");
    for d in [
        job.join("in"),
        job.join("out"),
        job.join("work"),
        other.clone(),
    ] {
        fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    }
    fs::write(job.join("in/probe.txt"), b"readable").map_err(|e| e.to_string())?;
    fs::write(job.join("probe.py"), PROBE).map_err(|e| e.to_string())?;
    let other_file = other.join("secret.txt");
    fs::write(&other_file, b"another job's data").map_err(|e| e.to_string())?;
    let sentinel = root.join("sentinel.txt");
    fs::write(&sentinel, b"outside every job").map_err(|e| e.to_string())?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let config = serde_json::json!({
        "sentinel": canonical(&sentinel),
        "home": canonical(home),
        "otherJobFile": canonical(&other_file),
        "outsideFile": canonical(&other).join("written.txt"),
        "loopbackPort": port,
    });
    let access = run(
        launcher,
        &job,
        ACCESS_LIMIT,
        vec!["access".into(), config.to_string().into()],
    )?;
    drop(listener);
    let report: BTreeMap<String, Value> = serde_json::from_slice(&access.stdout)
        .map_err(|_| "the probe produced no report".to_string())?;

    let mut attempts = Vec::new();
    for (names, expected) in [(DENIED, "blocked"), (ALLOWED, "allowed")] {
        for name in names {
            let outcome = report
                .get(*name)
                .and_then(|v| v["outcome"].as_str())
                .unwrap_or("missing")
                .to_owned();
            attempts.push(Attempt {
                name: (*name).to_owned(),
                expected,
                passed: outcome == expected,
                outcome,
            });
        }
    }
    // The probe must not have written outside, whatever it reported.
    attempts.push(Attempt {
        name: "nothing_written_outside".into(),
        expected: "absent",
        outcome: if other.join("written.txt").exists() {
            "present"
        } else {
            "absent"
        }
        .into(),
        passed: !other.join("written.txt").exists(),
    });

    let alloc = run(launcher, &job, ALLOC_LIMIT, vec!["alloc".into()])?;
    let reported = String::from_utf8_lossy(&alloc.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .any(|m| m["type"] == "error" && m["code"] == "E_RESOURCE_MEMORY");
    attempts.push(Attempt {
        name: "memory_limit".into(),
        expected: "E_RESOURCE_MEMORY",
        outcome: if reported {
            "E_RESOURCE_MEMORY".into()
        } else {
            format!("exit {:?}", alloc.status.code())
        },
        passed: reported && !alloc.status.success(),
    });

    let _ = fs::remove_dir_all(&root);
    let passed = attempts.iter().all(|a| a.passed);
    Ok(ProbeReport { attempts, passed })
}

fn run(
    launcher: &SandboxLauncher,
    job: &Path,
    limit: u64,
    args: Vec<OsString>,
) -> Result<std::process::Output, String> {
    let mut argv = vec![
        launcher.config().python.clone().into_os_string(),
        "-I".into(),
        canonical(job).join("probe.py").into_os_string(),
    ];
    argv.extend(args);
    let mut cmd = launcher.wrap(PROBE_JOB_ID, job, limit, argv)?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd.output()
        .map_err(|e| format!("can't start the probe: {e}"))
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}
