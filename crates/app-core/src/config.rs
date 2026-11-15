//! Finding the worker runtime, adapters and engine catalog: shared by the
//! desktop app and the `lfcs` command line.
//!
//! Two layouts exist:
//! - **bundled** (packaged builds): `<resources>/python`, `<resources>/adapters`
//!   and `<resources>/engines/engines.yaml`;
//! - **development**: the repository, with its `.venv` as the runtime.

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
pub fn find_dev_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| is_dev_root(d))
        .map(Path::to_path_buf)
}

/// A bundle: the resources folder holds its own Python runtime.
fn is_bundle(dir: &Path) -> bool {
    bundled_python(dir).is_file()
}

impl AppConfig {
    /// Builds the configuration for a process that is its own sandbox runner.
    ///
    /// With `resources`, that folder is used: the bundled layout when it holds
    /// a `python/` runtime, else a development checkout (its `.venv`). Without,
    /// the nearest checkout above the executable, then above the current
    /// directory, is used.
    pub fn locate(
        resources: Option<PathBuf>,
        runner: PathBuf,
        runner_args: Vec<OsString>,
    ) -> Result<AppConfig, String> {
        let exe_dir = runner.parent().map(Path::to_path_buf);
        let cwd = std::env::current_dir().ok();
        Self::locate_from(resources, runner, runner_args, exe_dir, cwd)
    }

    fn locate_from(
        resources: Option<PathBuf>,
        runner: PathBuf,
        runner_args: Vec<OsString>,
        exe_dir: Option<PathBuf>,
        cwd: Option<PathBuf>,
    ) -> Result<AppConfig, String> {
        let (resources, python) = match resources {
            // A bundle is used as given; a checkout is canonicalized (the
            // sandbox wants absolute, symlink-free paths).
            Some(res) if is_bundle(&res) => {
                let python = bundled_python(&res);
                (res, python)
            }
            Some(res) => {
                let root = res
                    .canonicalize()
                    .map_err(|e| format!("resources folder {}: {e}", res.display()))?;
                let python = venv_python(&root);
                (root, python)
            }
            None => {
                let root = exe_dir
                    .as_deref()
                    .and_then(find_dev_root)
                    .or_else(|| cwd.as_deref().and_then(find_dev_root))
                    .ok_or(
                        "can't find the converter's files (pass --resources, or run from the repository)",
                    )?;
                let root = root.canonicalize().unwrap_or(root);
                let python = venv_python(&root);
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

    #[test]
    fn dev_layout_from_the_exe_dir() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(repo.join("adapters")).unwrap();
        touch(&venv_python(&repo));
        let deep = repo.join("target/debug");
        fs::create_dir_all(&deep).unwrap();
        let c = AppConfig::locate_from(
            None,
            deep.join("lfcs"),
            vec!["--sandbox-run".into()],
            Some(deep),
            None,
        )
        .unwrap();
        let repo = repo.canonicalize().unwrap_or(repo);
        assert_eq!(c.adapters_dir, repo.join("adapters"));
        assert_eq!(c.python, venv_python(&repo));
        assert_eq!(c.runner_args, vec![OsString::from("--sandbox-run")]);
    }

    #[test]
    fn explicit_bundle_resources() {
        let t = tempfile::tempdir().unwrap();
        let res = t.path().join("res");
        touch(&bundled_python(&res));
        let c = AppConfig::locate(Some(res.clone()), "x".into(), vec![]).unwrap();
        assert_eq!(c.python, bundled_python(&res));
        assert_eq!(c.resources, res);
        assert_eq!(c.adapters_dir, res.join("adapters"));
    }

    #[test]
    fn explicit_dev_resources() {
        let t = tempfile::tempdir().unwrap();
        touch(&venv_python(t.path()));
        let c = AppConfig::locate(Some(t.path().into()), "x".into(), vec![]).unwrap();
        assert_eq!(c.python, venv_python(&t.path().canonicalize().unwrap()));
    }

    #[test]
    fn missing_runtime_or_folder_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        let e = AppConfig::locate(Some(t.path().into()), "x".into(), vec![]).unwrap_err();
        assert!(e.contains("worker runtime is missing"), "{e}");
        let e = AppConfig::locate(Some(t.path().join("nope")), "x".into(), vec![]).unwrap_err();
        assert!(e.contains("resources folder"), "{e}");
        let e = AppConfig::locate_from(None, "x".into(), vec![], Some(t.path().into()), None)
            .unwrap_err();
        assert!(e.contains("--resources"), "{e}");
    }
}
