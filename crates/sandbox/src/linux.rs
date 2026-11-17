//! Linux: bubblewrap (spec 9.2, change C7).
//!
//! `--unshare-all` gives empty network, PID, IPC, UTS and user namespaces;
//! only the runtime is bound read-only and the job directory read-write.
//! On Ubuntu 24.04, unprivileged user namespaces are restricted by AppArmor;
//! the `.deb` ships a profile for bwrap (C7). Elsewhere a failure here must
//! be reported, never worked around by running unsandboxed.
//!
//! Beyond the namespaces: `--cap-drop ALL`, `--disable-userns` (bwrap 0.8 and
//! newer; stops a worker creating a nested user namespace, the usual way to
//! regain capabilities), and shells masked with `/dev/null` so a worker can't
//! start one (the probe's `spawn_shell`). The runner adds a task cap
//! (`RLIMIT_NPROC`). **Still missing: a seccomp filter** (it needs a crate
//! that isn't approved), so the syscall surface is the kernel's own.
//!
//! **Not yet verified on a Linux machine** — the probe tests run on Linux CI
//! only where bwrap can create namespaces, and skip otherwise. The argument
//! list is unit-tested on every platform.

// Compiled on every Unix for its tests; only Linux uses it.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::Inner;

const CANDIDATES: &[&str] = &["/usr/bin/bwrap", "/bin/bwrap"];

/// System locations bound read-only so the interpreter and its libraries load.
const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/etc/ld.so.cache",
    "/etc/localtime",
];

/// Shells a worker has no business starting. Each is bound over with
/// `/dev/null`, which can't be executed. Resolved first: `/usr/bin/sh` is a
/// symlink, and the mount must land on the file it points to.
const SHELLS: &[&str] = &[
    "/usr/bin/sh",
    "/bin/sh",
    "/usr/bin/dash",
    "/usr/bin/bash",
    "/usr/bin/ash",
];

/// `bwrap --version` prints `bubblewrap 0.9.0`.
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let v = text.split_whitespace().last()?;
    let mut parts = v.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn bwrap_version(bwrap: &str) -> Option<(u32, u32)> {
    static VERSION: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let out = std::process::Command::new(bwrap)
            .arg("--version")
            .output()
            .ok()?;
        parse_version(&String::from_utf8_lossy(&out.stdout))
    })
}

pub(crate) fn command(inner: &Inner<'_>) -> Result<Vec<OsString>, String> {
    let bwrap = CANDIDATES
        .iter()
        .find(|p| Path::new(p).exists())
        .ok_or("bubblewrap (bwrap) is not installed")?;
    let masked: Vec<PathBuf> = SHELLS
        .iter()
        .filter_map(|s| Path::new(s).canonicalize().ok())
        .filter(|p| p.starts_with("/usr") && p.is_file())
        .collect();
    Ok(args(inner, bwrap, bwrap_version(bwrap), &masked))
}

fn args(
    inner: &Inner<'_>,
    bwrap: &str,
    version: Option<(u32, u32)>,
    masked: &[PathBuf],
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        bwrap.into(),
        "--unshare-all".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
        // No capabilities even if the worker somehow gains root in its
        // namespace.
        "--cap-drop".into(),
        "ALL".into(),
        "--clearenv".into(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ];
    for p in SYSTEM_READ {
        if Path::new(p).exists() {
            argv.extend(["--ro-bind".into(), (*p).into(), (*p).into()]);
        }
    }
    for p in inner
        .config
        .runtime_read
        .iter()
        .chain(std::iter::once(&inner.config.adapters_dir))
    {
        argv.extend(["--ro-bind".into(), p.clone().into(), p.clone().into()]);
    }
    for p in inner.grant.map(|g| g.read.as_slice()).unwrap_or_default() {
        argv.extend(["--ro-bind".into(), p.clone().into(), p.clone().into()]);
    }
    // After every bind of /usr (the runtime may be a system Python rooted
    // there): a later bind of the parent would hide the mask.
    for shell in masked {
        argv.extend(["--ro-bind".into(), "/dev/null".into(), shell.into()]);
    }
    let job = inner.job_dir.clone().into_os_string();
    argv.extend([
        "--bind".into(),
        job.clone(),
        job.clone(),
        "--chdir".into(),
        job,
    ]);
    // --clearenv wiped the runner's environment; set it again inside.
    for (key, value) in crate::worker_env(&inner.job_dir) {
        argv.extend(["--setenv".into(), key.into(), value]);
    }
    for (key, value) in inner.grant.map(|g| g.env.as_slice()).unwrap_or_default() {
        argv.extend(["--setenv".into(), key.into(), value.clone()]);
    }
    // Older bwrap rejects the flag, so it is only added where it exists.
    if version.is_some_and(|v| v >= (0, 8)) {
        argv.push("--disable-userns".into());
    }
    argv.push("--".into());
    argv.extend(inner.argv.iter().cloned());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SandboxConfig;

    fn argv(version: Option<(u32, u32)>, masked: &[PathBuf]) -> Vec<String> {
        let config = SandboxConfig {
            python: "/py/bin/python".into(),
            runtime_read: vec!["/py".into()],
            exec_roots: vec!["/py".into()],
            adapters_dir: "/app/adapters".into(),
            runner: "/app/run".into(),
            runner_args: vec![],
            interpreter_processes: 0,
            grants: Default::default(),
        };
        let inner = Inner {
            config: &config,
            job_dir: PathBuf::from("/data/jobs/j1"),
            argv: vec!["/py/bin/python".into()],
            grant: None,
        };
        args(&inner, "/usr/bin/bwrap", version, masked)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn parses_bwrap_versions() {
        assert_eq!(parse_version("bubblewrap 0.9.0\n"), Some((0, 9)));
        assert_eq!(parse_version("bubblewrap 0.4.1"), Some((0, 4)));
        assert_eq!(parse_version("bubblewrap 1.0"), Some((1, 0)));
        assert_eq!(parse_version("garbage"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn drops_capabilities_always_and_userns_when_supported() {
        let a = argv(Some((0, 4)), &[]);
        let i = a.iter().position(|x| x == "--cap-drop").unwrap();
        assert_eq!(a[i + 1], "ALL");
        assert!(!a.contains(&"--disable-userns".to_string()));
        assert!(!argv(None, &[]).contains(&"--disable-userns".to_string()));
        for v in [(0, 8), (0, 11), (1, 0)] {
            let a = argv(Some(v), &[]);
            assert!(a.contains(&"--disable-userns".to_string()), "{v:?}");
            // Before the program, or bwrap would pass it to the worker.
            let flag = a.iter().position(|x| x == "--disable-userns").unwrap();
            let dashes = a.iter().position(|x| x == "--").unwrap();
            assert!(flag < dashes);
        }
    }

    #[test]
    fn shells_are_masked_after_every_runtime_bind() {
        let a = argv(Some((0, 9)), &[PathBuf::from("/usr/bin/dash")]);
        let usr = a.iter().rposition(|x| x == "/app/adapters").unwrap();
        let mask = a.iter().position(|x| x == "/usr/bin/dash").unwrap();
        assert!(usr < mask);
        assert_eq!(a[mask - 1], "/dev/null");
        assert_eq!(a[mask - 2], "--ro-bind");
    }
}
