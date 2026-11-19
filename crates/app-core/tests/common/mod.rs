//! Shared by the integration tests (each a plain `main`, `harness = false`,
//! which is also the sandbox runner when started with `--sandbox-run`).
//!
//! The tests need the workspace virtualenv (`uv sync`) and a working sandbox.
//! Locally a test that can't have them says so and passes; under `CI` it
//! fails, so a broken runner can't turn the suite green by skipping.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use converter_app_core::{App, AppConfig};

/// The repository root.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The workspace virtualenv's interpreter (it may not exist).
pub fn python() -> PathBuf {
    if cfg!(windows) {
        repo().join(".venv/Scripts/python.exe")
    } else {
        repo().join(".venv/bin/python")
    }
}

/// The user's home folder, for the sandbox self-test.
pub fn home() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .expect("HOME or USERPROFILE is set"),
    )
}

/// Running under continuous integration (`CI` set to anything but empty,
/// `0` or `false`).
pub fn ci() -> bool {
    std::env::var("CI").is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false"))
}

/// Whether the sandbox has a backend on this OS.
pub fn sandbox_os() -> bool {
    cfg!(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    ))
}

/// Locally: prints that `what` was skipped and why. Under `CI`: panics, since
/// CI installs everything these tests need.
pub fn skip_or_fail(what: &str, why: &str) {
    if ci() {
        panic!("{what}: {why}; a skip is a failure under CI");
    }
    println!("{what}: skipped ({why})");
}

/// The configuration every test app uses: this checkout's adapters and
/// catalog, the virtualenv interpreter, and the test binary itself as the
/// sandbox runner. Built by hand, so it needs no `LFCS_DEV`.
pub fn test_config(python: &Path, data_root: PathBuf) -> AppConfig {
    AppConfig {
        data_root: Some(data_root),
        resources: repo(),
        adapters_dir: repo().join("adapters"),
        python: python.to_path_buf(),
        runner: std::env::current_exe().unwrap(),
        runner_args: vec!["--sandbox-run".into()],
        user_memory_limit: None,
        include_dev_adapters: false,
    }
}

/// An app with no event sink and dummy preview tokens.
pub fn open_app(python: &Path, data_root: PathBuf) -> App {
    open_app_with(&test_config(python, data_root))
}

pub fn open_app_with(config: &AppConfig) -> App {
    App::open(
        config,
        Arc::new(|_| {}),
        Arc::new(|_| "00000000000000000000000000000000".to_owned()),
        Arc::new(|_| {}),
    )
    .expect("the app opens")
}
