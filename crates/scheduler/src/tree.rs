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
//! - After a job ends, [`kill_job_strays`] looks for processes whose
//!   environment carries the job's `LFCS_JOB=<id>` tag (set by the sandbox
//!   for every worker) and kills them. That catches a daemon that
//!   double-forked out of the tree, which no parent or group walk can
//!   follow. On Windows the Job Object already holds every descendant, so
//!   the hunt does nothing there.
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

/// Kills every process of the current user whose environment contains
/// exactly `LFCS_JOB=<job_id>`, and returns how many it signalled.
///
/// The tag is inherited through `fork` and `exec`, so it follows a worker's
/// descendants however they detached. It is matched as a whole entry: another
/// job's tag, or this one with something appended, doesn't match, and neither
/// does an argument that merely looks like it. The caller's own process is
/// never touched. Call it once the job's workers have exited, and not while
/// another worker of the same job is meant to keep running.
///
/// Best effort: a process that can't be read (another user's, or gone) is
/// skipped. Killing can fork more tagged children before the signal lands,
/// so the sweep repeats until a pass finds none.
#[cfg(unix)]
pub fn kill_job_strays(job_id: &str) -> usize {
    let tag = format!("LFCS_JOB={job_id}");
    let mut killed = 0;
    for round in 0..5 {
        let found = tagged::find(tag.as_bytes());
        if found.is_empty() {
            break;
        }
        for pid in found {
            // SAFETY: plain syscall; `find` never returns our own PID or 1.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                killed += 1;
            }
        }
        if round < 4 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    killed
}

/// Windows: the Job Object holds every descendant, so there is nothing to hunt.
#[cfg(not(unix))]
pub fn kill_job_strays(_job_id: &str) -> usize {
    0
}

/// Whether `entries` (NUL-separated, as in `/proc/<pid>/environ`) holds
/// `tag` as a whole entry.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn environ_has_tag(entries: &[u8], tag: &[u8]) -> bool {
    entries.split(|&b| b == 0).any(|e| e == tag)
}

/// Whether the environment in a macOS `KERN_PROCARGS2` buffer holds `tag`.
///
/// The buffer is `argc` (a native-endian `i32`), the executable path, NUL
/// padding, then `argc` argument strings and then the environment strings,
/// all NUL-terminated. The arguments are skipped by count so that an
/// argument equal to the tag isn't taken for an environment entry.
#[cfg(any(target_os = "macos", test))]
fn procargs2_has_tag(buf: &[u8], tag: &[u8]) -> bool {
    let Some((argc, rest)) = buf.split_first_chunk::<4>() else {
        return false;
    };
    let Ok(argc) = usize::try_from(i32::from_ne_bytes(*argc)) else {
        return false;
    };
    // The executable path, then the padding after it.
    let Some(end) = rest.iter().position(|&b| b == 0) else {
        return false;
    };
    let rest = &rest[end..];
    let start = rest.iter().position(|&b| b != 0).unwrap_or(rest.len());
    let mut strings = rest[start..].split(|&b| b == 0);
    for _ in 0..argc {
        if strings.next().is_none() {
            return false;
        }
    }
    strings.any(|e| e == tag)
}

#[cfg(target_os = "linux")]
mod tagged {
    use std::os::unix::fs::MetadataExt;

    use super::environ_has_tag;

    /// An environment larger than this isn't read further; the tag is set
    /// by the sandbox, near the start.
    const MAX_ENVIRON_BYTES: u64 = 4 * 1024 * 1024;

    /// Own processes, other than this one, with the tag in `/proc/<pid>/environ`.
    pub(super) fn find(tag: &[u8]) -> Vec<libc::pid_t> {
        use std::io::Read;
        let me = std::process::id();
        // SAFETY: plain syscall.
        let uid = unsafe { libc::geteuid() };
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return found;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == me || pid <= 1 {
                continue;
            }
            let dir = entry.path();
            if std::fs::metadata(&dir).is_ok_and(|m| m.uid() != uid) {
                continue;
            }
            let Ok(file) = std::fs::File::open(dir.join("environ")) else {
                continue;
            };
            let mut environ = Vec::new();
            if file
                .take(MAX_ENVIRON_BYTES)
                .read_to_end(&mut environ)
                .is_err()
            {
                continue;
            }
            if environ_has_tag(&environ, tag) {
                found.push(pid as libc::pid_t);
            }
        }
        found
    }
}

#[cfg(target_os = "macos")]
mod tagged {
    use super::procargs2_has_tag;

    // `proc_listallpids` and `proc_pidinfo` come from libproc, which libc
    // declares for macOS; nothing here is declared by hand.

    /// Own processes, other than this one, with the tag in their environment
    /// (read with `sysctl(KERN_PROCARGS2)`).
    pub(super) fn find(tag: &[u8]) -> Vec<libc::pid_t> {
        let me = std::process::id() as libc::pid_t;
        // SAFETY: plain syscall.
        let uid = unsafe { libc::geteuid() };
        let mut pids = all_pids();
        pids.retain(|&p| p != me && p > 1);
        let mut buf = vec![0u8; arg_max()];
        pids.into_iter()
            .filter(|&pid| owned_by(pid, uid) && has_tag(pid, tag, &mut buf))
            .collect()
    }

    fn all_pids() -> Vec<libc::pid_t> {
        // SAFETY: a null buffer asks for the count.
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(n) = usize::try_from(n) else {
            return Vec::new();
        };
        // Processes may start between the two calls.
        let mut pids: Vec<libc::pid_t> = vec![0; n + n / 4 + 64];
        let bytes =
            i32::try_from(pids.len() * std::mem::size_of::<libc::pid_t>()).unwrap_or(i32::MAX);
        // SAFETY: `pids` holds `bytes` bytes.
        let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(usize::try_from(got).unwrap_or(0));
        pids.retain(|&p| p > 0);
        pids
    }

    fn arg_max() -> usize {
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let mut max: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: `max` is `len` bytes.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&raw mut max).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 && max > 0 {
            max as usize
        } else {
            1 << 20
        }
    }

    /// Whether `pid` runs as `uid`. A process that can't be asked about
    /// isn't ours to kill.
    fn owned_by(pid: libc::pid_t, uid: libc::uid_t) -> bool {
        // SAFETY: zeroed is a valid proc_bsdinfo (plain integers).
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` is `size` bytes.
        let got = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        got == size && info.pbi_uid == uid
    }

    fn has_tag(pid: libc::pid_t, tag: &[u8], buf: &mut [u8]) -> bool {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut len = buf.len();
        // SAFETY: `buf` is `len` bytes; the kernel writes at most that.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        rc == 0 && procargs2_has_tag(&buf[..len.min(buf.len())], tag)
    }
}

/// Other Unix systems have no worker sandbox to tag processes; Windows has
/// the Job Object.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod tagged {
    pub(super) fn find(_tag: &[u8]) -> Vec<libc::pid_t> {
        Vec::new()
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

#[cfg(test)]
mod parse_tests {
    use super::{environ_has_tag, procargs2_has_tag};

    #[test]
    fn environ_entries_match_whole() {
        let env = b"A=1\0LFCS_JOB=j_1\0LFCS_JOB=j_12\0";
        assert!(environ_has_tag(env, b"LFCS_JOB=j_1"));
        assert!(environ_has_tag(env, b"LFCS_JOB=j_12"));
        assert!(!environ_has_tag(env, b"LFCS_JOB=j_"));
        assert!(!environ_has_tag(b"XLFCS_JOB=j_1\0", b"LFCS_JOB=j_1"));
    }

    fn procargs(argv: &[&str], env: &[&str]) -> Vec<u8> {
        let mut buf = (argv.len() as i32).to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/prog\0\0\0\0");
        for s in argv.iter().chain(env) {
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
        }
        buf
    }

    #[test]
    fn procargs2_skips_arguments_and_matches_whole_entries() {
        let tag = b"LFCS_JOB=j_1";
        assert!(procargs2_has_tag(
            &procargs(&["a", "b"], &["X=1", "LFCS_JOB=j_1"]),
            tag
        ));
        // The tag as an argument isn't an environment entry.
        assert!(!procargs2_has_tag(
            &procargs(&["a", "LFCS_JOB=j_1"], &["X=1"]),
            tag
        ));
        assert!(!procargs2_has_tag(
            &procargs(&["a"], &["LFCS_JOB=j_10"]),
            tag
        ));
        assert!(!procargs2_has_tag(&[1, 0], tag));
        assert!(!procargs2_has_tag(&procargs(&[], &[])[..3], tag));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::fd::AsRawFd;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{kill_job_strays, prepare};

    fn alive(pid: libc::pid_t) -> bool {
        // Signal 0 only checks; a zombie counts as gone (nothing runs in it).
        // SAFETY: plain syscall.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        let state = Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        !state.is_empty() && !state.starts_with('Z')
    }

    /// Polls with a generous deadline: the process table lags a kill.
    fn gone_within(pid: libc::pid_t, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !alive(pid)
    }

    fn pid_from(path: &std::path::Path) -> libc::pid_t {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "the pid file never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The sleeper is this test binary, not `/bin/sleep`: macOS hides the
    /// environment of its own (SIP-protected) programs from `KERN_PROCARGS2`,
    /// but not of a worker's interpreter.
    const SLEEPER: &str = "LFCS_TEST_SLEEPER";

    /// Does nothing in a normal run; started as a child with `SLEEPER` set,
    /// it is the long-lived process the hunt looks for.
    #[test]
    fn sleeper_helper() {
        if std::env::var_os(SLEEPER).is_some() {
            std::thread::sleep(Duration::from_secs(300));
        }
    }

    fn sleeper_args() -> (std::path::PathBuf, [&'static str; 3]) {
        (
            std::env::current_exe().unwrap(),
            ["--exact", "tree::tests::sleeper_helper", "--nocapture"],
        )
    }

    /// SEC-2: a daemon that double-forked is no one's child any more, yet
    /// still carries the tag. Other jobs' tags, near misses and a tag that
    /// is only an argument are left alone.
    #[test]
    fn the_stray_hunt_kills_tagged_daemons_and_nothing_else() {
        let id = format!("j_hunt_{}", std::process::id());
        let dir = tempfile::tempdir().unwrap();

        let daemon_pid_file = dir.path().join("daemon.pid");
        let (exe, args) = sleeper_args();
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "('{}' {} >/dev/null 2>&1 & echo $! > '{}') &",
                exe.display(),
                args.join(" "),
                daemon_pid_file.display()
            ))
            .env("LFCS_JOB", &id)
            .env(SLEEPER, "1")
            .status()
            .unwrap();
        assert!(status.success());
        let daemon = pid_from(&daemon_pid_file);

        let spawn_sleeper = |tag: Option<String>| {
            let (exe, args) = sleeper_args();
            let mut cmd = Command::new(exe);
            cmd.args(args).env(SLEEPER, "1");
            if let Some(t) = tag {
                cmd.env("LFCS_JOB", t);
            }
            cmd.spawn().unwrap()
        };
        let mut other_job = spawn_sleeper(Some(format!("{id}_other")));
        let mut longer = spawn_sleeper(Some(format!("x{id}")));
        let mut untagged = spawn_sleeper(None);
        // The tag appears in argv only, not in the environment.
        let (exe, args) = sleeper_args();
        let mut in_argv = Command::new(exe)
            .args(args)
            .arg(format!("LFCS_JOB={id}"))
            .env(SLEEPER, "1")
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));

        assert!(alive(daemon));
        assert!(kill_job_strays(&id) >= 1);
        assert!(
            gone_within(daemon, Duration::from_secs(30)),
            "daemon survived"
        );

        for (name, child) in [
            ("other job", &mut other_job),
            ("longer id", &mut longer),
            ("untagged", &mut untagged),
            ("argv only", &mut in_argv),
        ] {
            assert!(alive(child.id() as libc::pid_t), "{name} was killed");
            let _ = child.kill();
            let _ = child.wait();
        }
        // Nothing left to find.
        assert_eq!(kill_job_strays(&id), 0);
    }

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
