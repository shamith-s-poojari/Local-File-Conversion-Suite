//! Finding the worker runtime, adapters and engine catalog: shared by the
//! desktop app and the `lfcs` command line.
//!
//! Two layouts exist:
//! - **bundled** (packaged builds): `<resources>/python`, `<resources>/adapters`
//!   and `<resources>/engines/engines.yaml`;
//! - **development**: the repository, with its `.venv` as the runtime. This
//!   is opt-in (`--dev`, or `LFCS_DEV=1`): the interpreter found there runs
//!   with the user's rights outside the sandbox (`SandboxConfig::discover`),
//!   so a normal run must never pick one up from wherever it happens to
//!   start. Even then only the executable's own folders are searched, never
//!   the current directory or its parents (a downloaded folder with a
//!   `.venv` would otherwise be a way to run code), and the interpreter must
//!   sit in a virtualenv inside the repository, owned by the user.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::AppConfig;

fn venv_python(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.join(".venv/Scripts/python.exe")
    } else {
        root.join(".venv/bin/python")
    }
}

fn bundled_python(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.join("python/python.exe")
    } else {
        root.join("python/bin/python3")
    }
}

/// A repository checkout: has `adapters/` and a virtualenv.
fn is_dev_root(dir: &Path) -> bool {
    dir.join("adapters").is_dir() && dir.join(".venv").is_dir()
}

/// The nearest directory at or above `start` that is a development checkout.
/// A relative `start` finds nothing: its ancestors would be resolved against
/// the current directory.
pub fn find_dev_root(start: &Path) -> Option<PathBuf> {
    if !start.is_absolute() {
        return None;
    }
    start
        .ancestors()
        .find(|d| is_dev_root(d))
        .map(Path::to_path_buf)
}

/// A bundle: the resources folder holds its own Python runtime.
fn is_bundle(dir: &Path) -> bool {
    bundled_python(dir).is_file()
}

/// Whether development mode was asked for through the environment.
fn dev_from_env() -> bool {
    dev_flag(std::env::var("LFCS_DEV").ok().as_deref())
}

fn dev_flag(value: Option<&str>) -> bool {
    value == Some("1")
}

/// A development runtime must be what it looks like: a virtualenv inside the
/// repository (not a link out of it) and owned by the current user. The
/// interpreter itself is usually a symlink to a system Python, so it is
/// judged where it sits, not where it points.
fn check_dev_runtime(root: &Path, python: &Path) -> Result<(), String> {
    let venv = root.join(".venv");
    let inside = venv.canonicalize().is_ok_and(|real| real.starts_with(root));
    if !inside || !python.starts_with(&venv) {
        return Err("the development runtime is not inside the repository".into());
    }
    let owned = |p: &Path| {
        std::fs::symlink_metadata(p).is_ok_and(|m| converter_sandbox::owned_by_current_user(&m))
    };
    if ![root, venv.as_path(), python].into_iter().all(owned) {
        return Err("the development runtime is not owned by the current user".into());
    }
    Ok(())
}

impl AppConfig {
    /// [`AppConfig::locate_with`], with development mode from `LFCS_DEV=1`.
    pub fn locate(
        resources: Option<PathBuf>,
        runner: PathBuf,
        runner_args: Vec<OsString>,
    ) -> Result<AppConfig, String> {
        Self::locate_with(resources, runner, runner_args, dev_from_env())
    }

    /// Builds the configuration for a process that is its own sandbox runner.
    ///
    /// With `resources`, that folder is used: the bundled layout when it holds
    /// a `python/` runtime, else (only with `dev`) a development checkout, its
    /// `.venv`. Without `resources`, only with `dev`, the nearest checkout
    /// above the executable is used. The current directory never is.
    pub fn locate_with(
        resources: Option<PathBuf>,
        runner: PathBuf,
        runner_args: Vec<OsString>,
        dev: bool,
    ) -> Result<AppConfig, String> {
        let exe_dir = runner.parent().map(Path::to_path_buf);
        Self::locate_from(resources, runner, runner_args, exe_dir, dev)
    }

    fn locate_from(
        resources: Option<PathBuf>,
        runner: PathBuf,
        runner_args: Vec<OsString>,
        exe_dir: Option<PathBuf>,
        dev: bool,
    ) -> Result<AppConfig, String> {
        let (resources, python) = match resources {
            // A bundle is used as given; a checkout is canonicalized (the
            // sandbox wants absolute, symlink-free paths).
            Some(res) if is_bundle(&res) => {
                let python = bundled_python(&res);
                (res, python)
            }
            Some(_) if !dev => {
                return Err(
                    "the resources folder has no worker runtime (development mode is off: pass --dev or set LFCS_DEV=1 to use a repository checkout)"
                        .into(),
                );
            }
            Some(res) => {
                let root = res
                    .canonicalize()
                    .map_err(|e| format!("resources folder {}: {e}", res.display()))?;
                let python = venv_python(&root);
                // A missing interpreter is reported below, as before.
                if python.is_file() {
                    check_dev_runtime(&root, &python)?;
                }
                (root, python)
            }
            None => {
                let root = dev
                    .then(|| exe_dir.as_deref().and_then(find_dev_root))
                    .flatten()
                    .ok_or(
                        "can't find the converter's files (pass --resources; a repository checkout also needs --dev or LFCS_DEV=1)",
                    )?;
                let root = root.canonicalize().unwrap_or(root);
                let python = venv_python(&root);
                // A missing interpreter is reported below, as before.
                if python.is_file() {
                    check_dev_runtime(&root, &python)?;
                }
                (root, python)
            }
        };
        if !python.is_file() {
            return Err("the worker runtime is missing (run `uv sync` in development)".into());
        }
        Ok(AppConfig {
            data_root: None,
            adapters_dir: resources.join("adapters"),
            resources,
            python,
            runner,
            runner_args,
            user_memory_limit: None,
            include_dev_adapters: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"").unwrap();
    }

    #[test]
    fn walks_up_to_a_checkout() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(repo.join("adapters")).unwrap();
        fs::create_dir_all(repo.join(".venv")).unwrap();
        let deep = repo.join("target/debug");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_dev_root(&deep).as_deref(), Some(repo.as_path()));
        assert!(find_dev_root(t.path()).is_none() || !is_dev_root(t.path()));
    }

    /// A checkout with a `.venv` interpreter, in a fresh temp folder.
    fn checkout(t: &Path, name: &str) -> PathBuf {
        let repo = t.join(name);
        fs::create_dir_all(repo.join("adapters")).unwrap();
        touch(&venv_python(&repo));
        repo
    }

    #[test]
    fn dev_layout_from_the_exe_dir() {
        let t = tempfile::tempdir().unwrap();
        let repo = checkout(t.path(), "repo");
        let deep = repo.join("target/debug");
        fs::create_dir_all(&deep).unwrap();
        let c = AppConfig::locate_from(
            None,
            deep.join("lfcs"),
            vec!["--sandbox-run".into()],
            Some(deep),
            true,
        )
        .unwrap();
        let repo = repo.canonicalize().unwrap_or(repo);
        assert_eq!(c.adapters_dir, repo.join("adapters"));
        assert_eq!(c.python, venv_python(&repo));
        assert_eq!(c.runner_args, vec![OsString::from("--sandbox-run")]);
    }

    #[test]
    fn development_mode_is_opt_in() {
        let t = tempfile::tempdir().unwrap();
        let repo = checkout(t.path(), "repo");
        let deep = repo.join("target/debug");
        fs::create_dir_all(&deep).unwrap();
        // The executable sits in a checkout, and still it is not used.
        let e =
            AppConfig::locate_from(None, deep.join("lfcs"), vec![], Some(deep), false).unwrap_err();
        assert!(e.contains("--resources"), "{e}");
        // Nor is an explicit checkout given as resources.
        let e = AppConfig::locate_with(Some(repo), "x".into(), vec![], false).unwrap_err();
        assert!(e.contains("development mode is off"), "{e}");
    }

    #[test]
    fn dev_flag_means_exactly_one() {
        assert!(dev_flag(Some("1")));
        for v in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert!(!dev_flag(v), "{v:?}");
        }
    }

    /// A hostile folder in the working directory, with the exact layout of a
    /// checkout, must never be used: not with development mode off, and not
    /// with it on either, whatever the runner path (a relative one such as
    /// `lfcs` has the parent `""`, which would resolve against the cwd).
    #[test]
    fn a_hostile_working_directory_is_ignored() {
        let t = tempfile::tempdir().unwrap();
        let evil = checkout(t.path(), "evil");
        // The adapters folder and virtualenv the audit describes.
        fs::create_dir_all(evil.join("adapters/hostile")).unwrap();
        let before = std::env::current_dir().unwrap();
        std::env::set_current_dir(&evil).unwrap();
        let results = [false, true].map(|dev| {
            [
                AppConfig::locate_with(None, "lfcs".into(), vec![], dev),
                AppConfig::locate_with(None, "./lfcs".into(), vec![], dev),
                AppConfig::locate_with(None, "".into(), vec![], dev),
            ]
        });
        std::env::set_current_dir(before).unwrap();
        for r in results.into_iter().flatten() {
            assert!(r.is_err(), "{:?}", r.map(|c| c.python));
        }
        assert!(find_dev_root(Path::new("")).is_none());
        assert!(find_dev_root(Path::new("rel/dir")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_virtualenv_that_links_out_of_the_repo_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let elsewhere = checkout(t.path(), "elsewhere");
        let repo = t.path().join("repo");
        fs::create_dir_all(repo.join("adapters")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join(".venv"), repo.join(".venv")).unwrap();
        let e = AppConfig::locate_with(Some(repo), "x".into(), vec![], true).unwrap_err();
        assert!(e.contains("not inside the repository"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_venv_interpreter_linked_to_the_system_python_is_accepted() {
        // The normal case: `.venv/bin/python` points at a Python elsewhere.
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(repo.join("adapters")).unwrap();
        let system = t.path().join("system/python3");
        touch(&system);
        let link = venv_python(&repo);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&system, &link).unwrap();
        let c = AppConfig::locate_with(Some(repo.clone()), "x".into(), vec![], true).unwrap();
        assert_eq!(c.python, venv_python(&repo.canonicalize().unwrap()));
    }

    #[test]
    fn explicit_bundle_resources() {
        let t = tempfile::tempdir().unwrap();
        let res = t.path().join("res");
        touch(&bundled_python(&res));
        let c = AppConfig::locate_with(Some(res.clone()), "x".into(), vec![], false).unwrap();
        assert_eq!(c.python, bundled_python(&res));
        assert_eq!(c.resources, res);
        assert_eq!(c.adapters_dir, res.join("adapters"));
    }

    #[test]
    fn explicit_dev_resources() {
        let t = tempfile::tempdir().unwrap();
        touch(&venv_python(t.path()));
        let c = AppConfig::locate_with(Some(t.path().into()), "x".into(), vec![], true).unwrap();
        assert_eq!(c.python, venv_python(&t.path().canonicalize().unwrap()));
    }

    #[test]
    fn missing_runtime_or_folder_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        let e =
            AppConfig::locate_with(Some(t.path().into()), "x".into(), vec![], true).unwrap_err();
        assert!(e.contains("worker runtime is missing"), "{e}");
        let e = AppConfig::locate_with(Some(t.path().join("nope")), "x".into(), vec![], true)
            .unwrap_err();
        assert!(e.contains("resources folder"), "{e}");
        let e = AppConfig::locate_from(None, "x".into(), vec![], Some(t.path().into()), true)
            .unwrap_err();
        assert!(e.contains("--resources"), "{e}");
    }
}
