//! Linux: bubblewrap (spec 9.2, change C7).
//!
//! `--unshare-all` gives empty network, PID, IPC, UTS and user namespaces;
//! only the runtime is bound read-only and the job directory read-write.
//! On Ubuntu 24.04, unprivileged user namespaces are restricted by AppArmor;
//! the `.deb` ships a profile for bwrap (C7). Elsewhere a failure here must
//! be reported, never worked around by running unsandboxed.
//!
//! **Not yet verified on a Linux machine** — the probe tests run on Linux CI
//! only where bwrap can create namespaces, and skip otherwise.

use std::ffi::OsString;
use std::path::Path;

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

pub(crate) fn command(inner: &Inner<'_>) -> Result<Vec<OsString>, String> {
    let bwrap = CANDIDATES
        .iter()
        .find(|p| Path::new(p).exists())
        .ok_or("bubblewrap (bwrap) is not installed")?;
    let mut argv: Vec<OsString> = vec![
        (*bwrap).into(),
        "--unshare-all".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
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
    argv.push("--".into());
    argv.extend(inner.argv.iter().cloned());
    Ok(argv)
}
