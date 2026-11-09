//! Job directory leases (spec section 10.4).
//!
//! A lease names the process that owns a job directory by PID **and** process
//! start time, because PIDs get reused. At startup, directories whose owner
//! is gone are collected.

use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// File name of the lease inside a job directory.
pub(crate) const LEASE_FILE: &str = ".lease";

/// Start times come from the OS in whole seconds and can be derived from the
/// boot time, so allow a little slack. A different process would have to
/// reuse the PID within this window to be mistaken for the owner.
const START_TIME_SLACK_S: u64 = 2;

/// The owner of a job directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lease {
    pub pid: u32,
    /// Process start time, in seconds since the Unix epoch.
    pub start_time: u64,
}

impl Lease {
    /// The lease for the current process.
    pub fn current() -> io::Result<Lease> {
        let pid = std::process::id();
        start_time_of(pid)
            .map(|start_time| Lease { pid, start_time })
            .ok_or_else(|| io::Error::other("can't read this process's start time"))
    }

    /// True if the owning process is still running.
    pub fn is_alive(&self) -> bool {
        match start_time_of(self.pid) {
            Some(t) => t.abs_diff(self.start_time) <= START_TIME_SLACK_S,
            None => false,
        }
    }

    /// Writes the lease into `dir` atomically.
    pub(crate) fn write_to(&self, dir: &Path) -> io::Result<()> {
        let mut tmp = tempfile::Builder::new()
            .prefix(".lease-")
            .tempfile_in(dir)?;
        tmp.write_all(&serde_json::to_vec(self).map_err(io::Error::other)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(dir.join(LEASE_FILE)).map_err(|e| e.error)?;
        Ok(())
    }

    /// Reads the lease in `dir`. `None` if it's missing or unreadable.
    pub(crate) fn read_from(dir: &Path) -> Option<Lease> {
        let path = dir.join(LEASE_FILE);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > 1024 {
            return None;
        }
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}

fn start_time_of(pid: u32) -> Option<u64> {
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid).map(|p| p.start_time())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        let lease = Lease::current().unwrap();
        assert!(lease.is_alive());
    }

    #[test]
    fn reused_pid_with_other_start_time_is_dead() {
        let mut lease = Lease::current().unwrap();
        lease.start_time = lease.start_time.saturating_sub(3600);
        assert!(!lease.is_alive());
    }

    #[test]
    fn exited_process_is_dead() {
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
        } else {
            std::process::Command::new("true").spawn()
        }
        .unwrap();
        let pid = child.id();
        let start_time = start_time_of(pid).unwrap_or(0);
        child.wait().unwrap();
        drop(child); // releases the handle, so Windows can retire the PID
        assert!(!Lease { pid, start_time }.is_alive());
    }

    #[test]
    fn round_trip_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Lease::read_from(dir.path()), None);
        let lease = Lease::current().unwrap();
        lease.write_to(dir.path()).unwrap();
        assert_eq!(Lease::read_from(dir.path()), Some(lease));
        std::fs::write(dir.path().join(LEASE_FILE), b"garbage").unwrap();
        assert_eq!(Lease::read_from(dir.path()), None);
    }
}
