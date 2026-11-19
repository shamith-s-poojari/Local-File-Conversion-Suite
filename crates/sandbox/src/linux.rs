//! Linux: bubblewrap (spec 9.2, change C7).
//!
//! `--unshare-all` gives empty network, PID, IPC, UTS and user namespaces;
//! only the runtime is bound read-only and the job directory read-write.
//! On Ubuntu 24.04, unprivileged user namespaces are restricted by AppArmor
//! (CI lifts the restriction; the `.deb` doesn't ship a bwrap profile yet).
//! A failure here must be reported, never worked around by running
//! unsandboxed.
//!
//! Beyond the namespaces: `--cap-drop ALL`, `--disable-userns` (bwrap 0.8 and
//! newer; stops a worker creating a nested user namespace, the usual way to
//! regain capabilities), and shells masked with `/dev/null` so a worker can't
//! start one (the probe's `spawn_shell`). The runner adds a task cap
//! (`RLIMIT_NPROC`). A small deny-list seccomp filter (`seccomp.rs`) goes in
//! through `--seccomp`: the runner makes the program's file descriptor (see
//! [`SECCOMP_FD`]) because it is the one that starts bwrap.
//!
//! Verified on Ubuntu 24.04 in CI (x86-64): the probe, end-to-end jobs with
//! FFmpeg and LibreOffice, and the seccomp filter. Other distributions are
//! untested. The argument list is unit-tested on every platform.

// Compiled on every Unix for its tests; only Linux uses it.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::Inner;

/// The descriptor number bwrap reads its seccomp program from. The runner
/// (`--seccomp-fd`) puts the program there just before starting bwrap, so the
/// number is agreed in advance and nothing leaks to the other workers the
/// scheduler starts. Above the standard streams and the few the runner holds.
pub(crate) const SECCOMP_FD: i32 = 10;

/// Size of the private `/tmp`. Workers keep their files in the job folder
/// (`TMPDIR` points there); `/tmp` holds only an engine's sockets and lock
/// files, and a tmpfs is memory, so a worker mustn't fill it. bwrap 0.9+.
const TMP_SIZE: u64 = 256 * 1024 * 1024;

const CANDIDATES: &[&str] = &["/usr/bin/bwrap", "/bin/bwrap"];

/// System locations bound read-only so the interpreter and its libraries load.
const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/etc/ld.so.cache",
    "/etc/localtime",
    // Debian and Ubuntu reach some libraries through alternatives links
    // (`libblas.so.3` -> /etc/alternatives/… -> /usr/lib/…/blas/…), and
    // FFmpeg needs them. The folder holds only symlinks.
    "/etc/alternatives",
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
    ];
    // `--size` applies to the next tmpfs and is new in bwrap 0.9; older
    // versions reject it, so there /tmp stays as large as the machine allows.
    if version.is_some_and(|v| v >= (0, 9)) {
        argv.extend(["--size".into(), TMP_SIZE.to_string().into()]);
    }
    argv.extend(["--tmpfs".into(), "/tmp".into()]);
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
    // Only the resolved folders are mounted, so a link on the way to the
    // interpreter that lies outside them would be missing (uv's Python
    // folders link `cpython-3.13-…` to `cpython-3.13.x-…`). Recreate it.
    let bound: Vec<&Path> = SYSTEM_READ
        .iter()
        .map(Path::new)
        .chain(inner.config.runtime_read.iter().map(PathBuf::as_path))
        .chain(std::iter::once(inner.config.adapters_dir.as_path()))
        .collect();
    for (link, target) in &inner.config.python_links {
        if !bound.iter().any(|b| link.starts_with(b)) {
            argv.extend([
                "--symlink".into(),
                target.clone().into(),
                link.clone().into(),
            ]);
        }
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
    for (key, value) in crate::worker_env(inner.job_id, &inner.job_dir) {
        argv.extend(["--setenv".into(), key.into(), value]);
    }
    for (key, value) in inner.grant.map(|g| g.env.as_slice()).unwrap_or_default() {
        argv.extend(["--setenv".into(), key.into(), value.clone()]);
    }
    // Older bwrap rejects the flag, so it is only added where it exists.
    // bwrap refuses it without `--unshare-user`: `--unshare-all` only *tries*
    // a user namespace, and the flag needs one for certain.
    if version.is_some_and(|v| v >= (0, 8)) {
        argv.push("--unshare-user".into());
        argv.push("--disable-userns".into());
    }
    if crate::seccomp::host_arch().is_some() {
        argv.extend(["--seccomp".into(), SECCOMP_FD.to_string().into()]);
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
        argv_with(version, masked, Vec::new())
    }

    fn args_with_links(links: Vec<(PathBuf, PathBuf)>) -> Vec<String> {
        argv_with(Some((0, 9)), &[], links)
    }

    fn argv_with(
        version: Option<(u32, u32)>,
        masked: &[PathBuf],
        python_links: Vec<(PathBuf, PathBuf)>,
    ) -> Vec<String> {
        let config = SandboxConfig {
            python_links,
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
            job_id: "j1",
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
            // bwrap: "--disable-userns requires --unshare-user" (not -try).
            let user = a.iter().position(|x| x == "--unshare-user").unwrap();
            assert!(user < dashes, "{v:?}");
        }
    }

    #[test]
    fn tmp_is_size_limited_where_bwrap_can() {
        for v in [Some((0, 8)), Some((0, 4)), None] {
            assert!(!argv(v, &[]).contains(&"--size".to_string()), "{v:?}");
        }
        for v in [(0, 9), (0, 11), (1, 0)] {
            let a = argv(Some(v), &[]);
            let size = a.iter().position(|x| x == "--size").expect("--size");
            assert_eq!(a[size + 1], TMP_SIZE.to_string());
            // The flag belongs to the very next tmpfs: /tmp.
            assert_eq!(&a[size + 2..size + 4], ["--tmpfs", "/tmp"]);
        }
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn the_seccomp_filter_is_passed_before_the_program() {
        let a = argv(Some((0, 9)), &[]);
        let i = a.iter().position(|x| x == "--seccomp").expect("--seccomp");
        assert_eq!(a[i + 1], SECCOMP_FD.to_string());
        assert!(i < a.iter().position(|x| x == "--").unwrap());
    }

    #[test]
    fn the_worker_environment_carries_the_job_tag() {
        let a = argv(Some((0, 9)), &[]);
        let i = a.iter().position(|x| x == "LFCS_JOB").expect("LFCS_JOB");
        assert_eq!(a[i - 1], "--setenv");
        assert_eq!(a[i + 1], "j1");
    }

    #[test]
    fn links_to_the_interpreter_outside_the_mounts_are_recreated() {
        let link = PathBuf::from("/home/u/.local/share/uv/python/cpython-3.13-linux");
        let target = PathBuf::from("/home/u/.local/share/uv/python/cpython-3.13.9-linux");
        let inside = PathBuf::from("/usr/bin/python3");
        let a = args_with_links(vec![
            (inside.clone(), PathBuf::from("/usr/bin/python3.12")),
            (link.clone(), target.clone()),
        ]);
        let i = a
            .iter()
            .position(|x| x == "--symlink")
            .expect("a --symlink");
        assert_eq!(a[i + 1], target.to_string_lossy());
        assert_eq!(a[i + 2], link.to_string_lossy());
        // A link inside a mounted folder is already there.
        assert_eq!(a.iter().filter(|x| *x == "--symlink").count(), 1);
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
