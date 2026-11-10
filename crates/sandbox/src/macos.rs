//! macOS: a per-job Seatbelt profile run through `sandbox-exec` (spike S1, change C8).
//!
//! Per-job profiles were chosen over App Sandbox because App Sandbox children
//! inherit the parent's access, so one job could read another's files (C8,
//! criterion 1). `sandbox-exec` is deprecated but present and functional on
//! every macOS we target; Apple's own services still use Seatbelt profiles.
//!
//! The profile denies by default and allows:
//! - reading and mapping system libraries, the interpreter and site-packages,
//!   and the adapters (read-only);
//! - reading and writing the job directory;
//! - executing binaries inside the interpreter's install only (the
//!   framework launcher re-executes `Python.app`, and `pdf-redact` starts
//!   its verifier). `/bin/sh` and everything else is denied;
//! - metadata lookups anywhere (Python stats many paths at startup; this
//!   reveals names and sizes, never contents), and listing `/` itself.
//!
//! No network rule is allowed, so sockets, loopback and DNS all fail.

use std::ffi::OsString;
use std::path::Path;

use crate::Inner;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// System locations every process needs to load.
const SYSTEM_READ: &[&str] = &[
    "/System",
    "/usr/lib",
    "/usr/share",
    "/Library/Apple/System",
    "/private/var/db/dyld",
    "/private/var/db/timezone",
];

const DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/urandom",
    "/dev/random",
    "/dev/dtracehelper",
];

pub(crate) fn command(inner: &Inner<'_>) -> Result<Vec<OsString>, String> {
    if !Path::new(SANDBOX_EXEC).exists() {
        return Err("sandbox-exec is missing".into());
    }
    let profile = profile(inner)?;
    let mut argv: Vec<OsString> = vec![SANDBOX_EXEC.into(), "-p".into(), profile.into()];
    argv.extend(inner.argv.iter().cloned());
    Ok(argv)
}

pub(crate) fn profile(inner: &Inner<'_>) -> Result<String, String> {
    let mut read = Vec::new();
    for p in SYSTEM_READ {
        read.push(quote(Path::new(p))?);
    }
    let mut runtime = Vec::new();
    for p in inner
        .config
        .runtime_read
        .iter()
        .chain(std::iter::once(&inner.config.adapters_dir))
    {
        runtime.push(quote(p)?);
    }
    let mut exec = Vec::new();
    for p in &inner.config.exec_roots {
        exec.push(quote(p)?);
    }
    let job = quote(&inner.job_dir)?;
    let devices: Vec<String> = DEVICES
        .iter()
        .map(|d| format!("(literal \"{d}\")"))
        .collect();
    let subpaths = |v: &[String]| {
        v.iter()
            .map(|p| format!("(subpath {p})"))
            .collect::<Vec<_>>()
            .join(" ")
    };

    Ok(format!(
        "(version 1)\n\
         (deny default)\n\
         (allow process-fork)\n\
         (allow process-exec {exec})\n\
         (allow file-read-metadata)\n\
         (allow file-read-data (literal \"/\"))\n\
         (allow file-read* {read} {runtime})\n\
         (allow file-map-executable {read} {runtime})\n\
         (allow file-read* {devices})\n\
         (allow file-write-data (literal \"/dev/null\") (literal \"/dev/dtracehelper\"))\n\
         (allow file-read* file-write* (subpath {job}))\n\
         (allow sysctl-read)\n\
         (allow signal (target same-sandbox))\n",
        exec = subpaths(&exec),
        read = subpaths(&read),
        runtime = subpaths(&runtime),
        devices = devices.join(" "),
    ))
}

/// A path as an SBPL string literal. Paths must be absolute and canonical:
/// Seatbelt matches resolved paths (`/var` is `/private/var`).
fn quote(p: &Path) -> Result<String, String> {
    let s = p.to_str().ok_or("non-UTF-8 path")?;
    if !p.is_absolute() || s.contains('\0') || s.contains('\n') {
        return Err("unusable path for the sandbox profile".into());
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SandboxConfig;
    use std::path::PathBuf;

    #[test]
    fn profile_quotes_paths_and_scopes_the_job() {
        let config = SandboxConfig {
            python: "/py/bin/python".into(),
            runtime_read: vec!["/py".into()],
            exec_roots: vec!["/py".into()],
            adapters_dir: "/app/adapters".into(),
            runner: "/app/run".into(),
        };
        let inner = Inner {
            config: &config,
            job_dir: PathBuf::from("/data/jobs/j_\"1"),
            argv: vec![],
        };
        let p = profile(&inner).unwrap();
        assert!(p.starts_with("(version 1)\n(deny default)\n"));
        assert!(p.contains("(allow file-read* file-write* (subpath \"/data/jobs/j_\\\"1\"))"));
        assert!(p.contains("(allow process-exec (subpath \"/py\"))"));
        assert!(!p.contains("network"));
        let relative = Inner {
            config: &config,
            job_dir: PathBuf::from("jobs/x"),
            argv: vec![],
        };
        assert!(profile(&relative).is_err());
    }
}
