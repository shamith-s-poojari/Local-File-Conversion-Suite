//! Killing a worker's whole process tree (spec section 10.3).
//!
//! - Unix: the worker starts in its own process group, and the group is sent
//!   `SIGKILL`.
//! - Windows: the worker is put in a Job Object with "kill on close", and the
//!   job is terminated. If the core itself dies, the OS closes the handle and
//!   the tree dies with it.
//!
//! - macOS also hunts runaways ([`Strays`]): a descendant that called
//!   `setsid` or `setpgid` is outside the group, so the group kill misses it.
//!
//! Known gap on Windows: a child the worker starts before it's assigned to
//! the job escapes it. Workers don't start children that early; the sandbox
//! (T09) creates its processes suspended, which closes the gap.

use std::io;
use std::process::{Child, Command};

/// Configures a command so its tree can be killed later, and (Unix) so the
/// worker starts with no inherited descriptors and can't write a file larger
/// than `max_file_bytes`.
pub(crate) fn prepare(cmd: &mut Command, max_file_bytes: u64) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        // SAFETY: the closure runs between fork and exec, so it makes only
        // async-signal-safe calls: no allocation, no locks, no logging.
        unsafe {
            cmd.pre_exec(move || {
                limit_file_size(max_file_bytes)?;
                close_inherited_fds();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = max_file_bytes;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Caps the size of any file the worker writes (`RLIMIT_FSIZE`). Both limits
/// are lowered so the worker can't raise its own back. Beyond the cap a write
/// fails with `EFBIG` or the process gets `SIGXFSZ`.
#[cfg(unix)]
fn limit_file_size(max: u64) -> io::Result<()> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: plain syscalls on a local struct.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_FSIZE, &mut lim) != 0 {
            return Err(io::Error::last_os_error());
        }
        let want = libc::rlim_t::try_from(max).unwrap_or(libc::rlim_t::MAX);
        let want = want.min(lim.rlim_max);
        lim.rlim_cur = want;
        lim.rlim_max = want;
        if libc::setrlimit(libc::RLIMIT_FSIZE, &lim) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Marks every descriptor from 3 up close-on-exec, so the worker can't see
/// anything the core has open (a descriptor opened without `O_CLOEXEC` by
/// some library, say). Marking rather than closing keeps the pipe `std` uses
/// to report an exec failure usable until the exec itself.
///
/// Async-signal-safe: syscalls only.
#[cfg(unix)]
fn close_inherited_fds() {
    // SAFETY: plain syscalls; the descriptors are the child's own copies.
    unsafe {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            const CLOSE_RANGE_CLOEXEC: libc::c_uint = 4;
            let first: libc::c_uint = 3;
            if libc::syscall(
                libc::SYS_close_range,
                first,
                libc::c_uint::MAX,
                CLOSE_RANGE_CLOEXEC,
            ) == 0
            {
                return;
            }
            // Before Linux 5.11: walk the table instead.
        }
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let top = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
            // An unlimited table is not walked to its end; Rust and the
            // libraries we use set close-on-exec themselves above this.
            lim.rlim_cur.min(65_536) as libc::c_int
        } else {
            1024
        };
        for fd in 3..top {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// Descendants of a worker found by their parent and session rather than by
/// process group, to kill after the group.
#[derive(Default)]
pub(crate) struct Strays {
    #[cfg(target_os = "macos")]
    procs: Vec<strays::Proc>,
}

#[cfg(target_os = "macos")]
mod strays {
    use std::collections::HashSet;

    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    /// A process and when it started, so a recycled PID isn't killed by mistake.
    #[derive(Clone, Copy)]
    pub(super) struct Proc {
        pid: Pid,
        start: u64,
    }

    /// Every process below `root`, found by walking the whole process table
    /// (`proc_listallpids`, so there is no per-parent child cap): those whose
    /// parent is in the set, and those in a session any member started
    /// (other than the core's own). Repeats until nothing new turns up.
    pub(super) fn find(root: u32) -> Vec<Proc> {
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let me = Pid::from_u32(std::process::id());
        // SAFETY: getsid(0) reads the caller's own session.
        let own_session = unsafe { libc::getsid(0) };
        let root = Pid::from_u32(root);
        let mut set: HashSet<Pid> = HashSet::from([root]);
        let mut sessions: HashSet<Pid> = HashSet::new();
        loop {
            let before = set.len();
            for (&pid, p) in sys.processes() {
                if pid == me || pid.as_u32() <= 1 || set.contains(&pid) {
                    continue;
                }
                let child = p.parent().is_some_and(|pp| set.contains(&pp));
                let same_session = p.session_id().is_some_and(|s| sessions.contains(&s));
                if child || same_session {
                    set.insert(pid);
                }
            }
            for pid in &set {
                let session = sys.process(*pid).and_then(|p| p.session_id());
                if let Some(s) = session.filter(|s| s.as_u32() as libc::pid_t != own_session) {
                    sessions.insert(s);
                }
            }
            if set.len() == before {
                break;
            }
        }
        set.remove(&root);
        set.into_iter()
            .filter_map(|pid| {
                sys.process(pid).map(|p| Proc {
                    pid,
                    start: p.start_time(),
                })
            })
            .collect()
    }

    /// SIGKILLs the processes that are still the ones `find` saw.
    pub(super) fn kill(procs: &[Proc]) {
        if procs.is_empty() {
            return;
        }
        let pids: Vec<Pid> = procs.iter().map(|p| p.pid).collect();
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing(),
        );
        for p in procs {
            if sys
                .process(p.pid)
                .is_some_and(|q| q.start_time() == p.start)
            {
                // SAFETY: plain syscall.
                unsafe {
                    libc::kill(p.pid.as_u32() as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }
}

pub(crate) struct ProcessTree {
    #[cfg(unix)]
    pgid: i32,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

// The job handle is only used through thread-safe Win32 calls.
unsafe impl Send for ProcessTree {}

impl ProcessTree {
    /// Takes ownership of `child`'s tree. Call right after spawning.
    pub(crate) fn attach(child: &Child) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let pgid = i32::try_from(child.id()).map_err(io::Error::other)?;
            Ok(ProcessTree { pgid })
        }
        #[cfg(windows)]
        {
            windows::attach(child)
        }
    }

    /// Finds descendants that left the process group. Call while the worker
    /// is still alive: once it exits they're reparented and the link is gone.
    /// Always empty off macOS, where the sandbox's PID namespace or Job
    /// Object already covers them.
    pub(crate) fn find_strays(&self) -> Strays {
        #[cfg(target_os = "macos")]
        {
            Strays {
                procs: strays::find(self.pgid as u32),
            }
        }
        #[cfg(not(target_os = "macos"))]
        Strays::default()
    }

    /// Kills what [`find_strays`](Self::find_strays) found.
    pub(crate) fn kill_strays(&self, strays: &Strays) {
        #[cfg(target_os = "macos")]
        strays::kill(&strays.procs);
        #[cfg(not(target_os = "macos"))]
        let _ = strays;
    }

    /// Kills every process in the tree. Safe to call more than once.
    pub(crate) fn kill(&self) {
        let strays = self.find_strays();
        #[cfg(unix)]
        // SAFETY: plain syscall; a negative PID addresses the process group.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        self.kill_strays(&strays);
        #[cfg(windows)]
        // SAFETY: the handle is valid until drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.kill();
        #[cfg(windows)]
        // SAFETY: closing our own handle once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    use super::ProcessTree;

    pub(super) fn attach(child: &Child) -> io::Result<ProcessTree> {
        // SAFETY: standard Job Object setup; every handle is checked and
        // closed on failure.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                let e = io::Error::last_os_error();
                CloseHandle(job);
                return Err(e);
            }
            Ok(ProcessTree { job })
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::fd::AsRawFd;
    use std::process::Command;

    use super::prepare;

    fn sh(script: &str, max_file_bytes: u64) -> std::process::ExitStatus {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(script);
        prepare(&mut cmd, max_file_bytes);
        cmd.status().unwrap()
    }

    #[test]
    fn inherited_descriptors_are_not_visible_to_the_worker() {
        // A descriptor that, unlike the ones `std` opens, survives exec.
        let file = std::fs::File::open("/dev/null").unwrap();
        let fd = file.as_raw_fd();
        // SAFETY: clearing a flag on a descriptor this test owns.
        unsafe {
            assert_eq!(libc::fcntl(fd, libc::F_SETFD, 0), 0);
        }
        // `: >&N` fails when N isn't open in the shell.
        let status = sh(
            &format!("(: >&{fd}) 2>/dev/null && exit 7; exit 0"),
            u64::MAX,
        );
        assert_eq!(status.code(), Some(0), "the worker saw fd {fd}");
        drop(file);
    }

    #[test]
    fn a_worker_cannot_write_past_the_file_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("big");
        let status = sh(
            &format!("head -c 2000000 /dev/zero > '{}'", target.display()),
            100_000,
        );
        assert!(!status.success());
        assert!(std::fs::metadata(&target).unwrap().len() <= 100_000);
    }
}
