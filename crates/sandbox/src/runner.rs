//! The worker runner: starts a sandboxed worker and enforces its memory limit.
//!
//! ```text
//! <runner> [prefix args] --memory-limit BYTES --job-id ID -- PROGRAM [ARGS...]
//! ```
//!
//! The worker inherits stdin, stdout and stderr, so the protocol flows
//! straight between it and the core. Every 50 ms the runner sums the
//! memory of the worker and its descendants (an RSS watchdog: spec
//! 9.2, since `RLIMIT_AS` is unreliable on macOS). Over the limit, it kills
//! them and writes a protocol `error` with `E_RESOURCE_MEMORY`, which the
//! scheduler treats like any worker error (retry once, alone, larger).
//!
//! The runner stays in the scheduler's process group, so killing the group
//! kills it and the worker together. Exit status mirrors the worker's.
//!
//! It runs as `converter-sandbox-run`, or as the desktop app itself started
//! with `--sandbox-run` (so releases ship no extra binary).

use std::io::Write;
use std::process::ExitCode;

pub(crate) const EX_USAGE: u8 = 64;
pub(crate) const EX_UNAVAILABLE: u8 = 69;
pub(crate) const KILLED_FOR_MEMORY: u8 = 137;

pub(crate) struct Args {
    pub memory_limit: u64,
    pub job_id: String,
    /// Processes the worker may run at once, itself included. Windows
    /// enforces it with the Job Object; Linux and macOS cap tasks with
    /// `RLIMIT_NPROC` (see `task_cap`).
    #[cfg_attr(
        all(unix, not(any(target_os = "linux", target_os = "macos"))),
        allow(dead_code)
    )]
    pub max_processes: u32,
    /// Linux: a descriptor number to fill with the seccomp program for
    /// bubblewrap's `--seccomp` before starting the program.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub seccomp_fd: Option<i32>,
    pub program: Vec<String>,
}

fn parse(args: Vec<String>) -> Option<Args> {
    let mut args = args.into_iter();
    let mut memory_limit = None;
    let mut job_id = None;
    let mut max_processes = 1;
    let mut seccomp_fd = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--memory-limit" => memory_limit = args.next()?.parse().ok(),
            "--job-id" => job_id = args.next(),
            "--max-processes" => max_processes = args.next()?.parse().ok().filter(|n| *n >= 1)?,
            "--seccomp-fd" => seccomp_fd = Some(args.next()?.parse().ok().filter(|n| *n > 2)?),
            "--" => break,
            _ => return None,
        }
    }
    let program: Vec<String> = args.collect();
    let job_id = job_id?;
    let valid_id = !job_id.is_empty()
        && job_id.len() <= 64
        && job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid_id || program.is_empty() {
        return None;
    }
    Some(Args {
        memory_limit: memory_limit?,
        job_id,
        max_processes,
        seccomp_fd,
        program,
    })
}

/// Runs the runner with these arguments (without the program name or any
/// prefix such as `--sandbox-run`).
pub fn main_with(args: Vec<String>) -> ExitCode {
    let Some(args) = parse(args) else {
        eprintln!(
            "usage: converter-sandbox-run --memory-limit N --job-id ID [--max-processes N] [--seccomp-fd N] -- PROGRAM ..."
        );
        return ExitCode::from(EX_USAGE);
    };
    #[cfg(unix)]
    return unix::main(args);
    #[cfg(windows)]
    return crate::windows::run(args);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = args;
        eprintln!("sandbox runner: no sandbox on this platform");
        ExitCode::from(EX_UNAVAILABLE)
    }
}

/// The protocol `error` the scheduler sees when the worker was killed for memory.
pub(crate) fn report_memory(job_id: &str, limit: u64) {
    let line = serde_json::json!({
        "v": 1,
        "type": "error",
        "jobId": job_id,
        "code": "E_RESOURCE_MEMORY",
        "retryable": true,
        "detail": { "limitBytes": limit },
    });
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[cfg(unix)]
mod unix {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Command, ExitCode};
    use std::thread;
    use std::time::Duration;

    use super::{report_memory, Args, EX_UNAVAILABLE, KILLED_FOR_MEMORY};

    const POLL: Duration = Duration::from_millis(50);

    /// `RLIMIT_NPROC` for a worker: what the user runs now, plus room per
    /// permitted process. Linux counts tasks, threads included (ffmpeg and
    /// LibreOffice start dozens); macOS counts processes. Either way the
    /// limit is checked against all of the user's processes system-wide, not
    /// the worker's own, so a small absolute number would break workers
    /// outright, and a user starting many programs during a job eats the
    /// headroom. The point is to stop a fork bomb, not to count processes:
    /// the memory watchdog and (Linux) the PID namespace do the precise work.
    /// Seatbelt has no process-count rule (it can only allow or deny
    /// `process-fork` altogether), so this is the only cap macOS has.
    /// Unix-wide so it can be tested anywhere.
    pub(super) fn task_cap(tasks_now: u64, max_processes: u32) -> u64 {
        tasks_now
            .saturating_add(256)
            .saturating_add(256 * u64::from(max_processes))
    }

    /// Kernel scheduling entities now: the fourth field of `/proc/loadavg`
    /// (`running/total`).
    #[cfg(target_os = "linux")]
    fn tasks_now() -> u64 {
        std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| {
                s.split_whitespace()
                    .nth(3)?
                    .split('/')
                    .nth(1)?
                    .parse::<u64>()
                    .ok()
            })
            // Unreadable /proc: a generous guess rather than no limit at all.
            .unwrap_or(32768)
    }

    /// The user's processes now, from `proc_listpids` (bytes needed, four per pid).
    #[cfg(target_os = "macos")]
    fn tasks_now() -> u64 {
        // PROC_UID_ONLY, <sys/proc_info.h>.
        const PROC_UID_ONLY: u32 = 4;
        // SAFETY: a null buffer asks only for the size.
        let bytes =
            unsafe { libc::proc_listpids(PROC_UID_ONLY, libc::geteuid(), std::ptr::null_mut(), 0) };
        u64::try_from(bytes).map_or(2048, |b| b / 4)
    }

    /// A descriptor holding the seccomp program, for bwrap to read.
    #[cfg(target_os = "linux")]
    fn seccomp_file() -> std::io::Result<std::fs::File> {
        use std::io::{Seek, Write};
        use std::os::unix::io::FromRawFd;
        let Some(arch) = crate::seccomp::host_arch() else {
            return Err(std::io::Error::other(
                "no seccomp table for this architecture",
            ));
        };
        // SAFETY: memfd_create with a NUL-terminated name; MFD_CLOEXEC (1) so
        // only the copy made for bwrap survives exec.
        let fd = unsafe { libc::syscall(libc::SYS_memfd_create, c"lfcs-seccomp".as_ptr(), 1u32) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a fresh descriptor we own.
        let mut file = unsafe { std::fs::File::from_raw_fd(fd as i32) };
        file.write_all(&crate::seccomp::program(arch))?;
        file.rewind()?;
        Ok(file)
    }

    pub fn main(args: Args) -> ExitCode {
        let mut cmd = Command::new(&args.program[0]);
        cmd.args(&args.program[1..]);
        let limit = args.memory_limit;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let nproc = task_cap(tasks_now(), args.max_processes) as libc::rlim_t;
        // Asked for, so never start without it.
        #[cfg(target_os = "linux")]
        let seccomp = match args.seccomp_fd.map(|fd| seccomp_file().map(|f| (f, fd))) {
            Some(Ok(pair)) => Some(pair),
            Some(Err(_)) => return ExitCode::from(EX_UNAVAILABLE),
            None => None,
        };
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                let none = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                libc::setrlimit(libc::RLIMIT_CORE, &none);
                #[cfg(target_os = "linux")]
                {
                    // Address space is a backstop only: libraries reserve far
                    // more than they touch, so allow generous headroom.
                    let cap = limit.saturating_mul(4).saturating_add(2 << 30);
                    let lim = libc::rlimit {
                        rlim_cur: cap,
                        rlim_max: cap,
                    };
                    libc::setrlimit(libc::RLIMIT_AS, &lim);
                    let procs = libc::rlimit {
                        rlim_cur: nproc,
                        rlim_max: nproc,
                    };
                    libc::setrlimit(libc::RLIMIT_NPROC, &procs);
                    if let Some((file, fd)) = &seccomp {
                        use std::os::unix::io::AsRawFd;
                        let from = file.as_raw_fd();
                        // dup2 gives the copy no close-on-exec flag; if the
                        // descriptor already is the wanted number, clear it.
                        if from == *fd {
                            libc::fcntl(from, libc::F_SETFD, 0);
                        } else if libc::dup2(from, *fd) < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    // The hard limit can't be raised again, so it also caps
                    // what descendants set. Never above what the system allows.
                    let mut cur: libc::rlimit = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::getrlimit(libc::RLIMIT_NPROC, &mut cur) == 0 {
                        let cap = nproc.min(cur.rlim_max);
                        let procs = libc::rlimit {
                            rlim_cur: cap,
                            rlim_max: cap,
                        };
                        libc::setrlimit(libc::RLIMIT_NPROC, &procs);
                    }
                }
                let _ = limit;
                Ok(())
            });
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return ExitCode::from(EX_UNAVAILABLE),
        };
        let root = child.id() as i32;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(code) = status.code() {
                        return ExitCode::from(code.clamp(0, 255) as u8);
                    }
                    let signal = status.signal().unwrap_or(9);
                    return ExitCode::from((128 + signal).clamp(0, 255) as u8);
                }
                Ok(None) => {}
                Err(_) => return ExitCode::from(EX_UNAVAILABLE),
            }
            let tree = descendants(root);
            let rss: u64 = tree.iter().map(|&p| resident_bytes(p)).sum();
            if rss > args.memory_limit {
                for &pid in tree.iter().rev() {
                    // SAFETY: plain syscall on PIDs we just found as our descendants.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
                let _ = child.wait();
                report_memory(&args.job_id, args.memory_limit);
                return ExitCode::from(KILLED_FOR_MEMORY);
            }
            thread::sleep(POLL);
        }
    }

    /// `root` and every process below it, parents first.
    fn descendants(root: i32) -> Vec<i32> {
        let mut out = vec![root];
        let mut i = 0;
        while i < out.len() && out.len() < 4096 {
            let pid = out[i];
            out.extend(children(pid));
            i += 1;
        }
        out
    }

    #[cfg(target_os = "macos")]
    fn children(pid: i32) -> Vec<i32> {
        let mut buf = vec![0i32; 256];
        let size = (buf.len() * std::mem::size_of::<i32>()) as libc::c_int;
        // SAFETY: the buffer is valid for `size` bytes; returns a PID count.
        let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), size) };
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize);
        buf.retain(|&p| p > 0);
        buf
    }

    /// The process's physical footprint: its private and compressed pages,
    /// what Activity Monitor shows as Memory. Resident size would also count
    /// the shared system frameworks a GUI engine maps (LibreOffice links
    /// AppKit), which cost nothing extra and roughly double its reading.
    #[cfg(target_os = "macos")]
    fn resident_bytes(pid: i32) -> u64 {
        // SAFETY: zeroed is a valid rusage_info_v2 (plain integers).
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a valid RUSAGE_INFO_V2 buffer for the call.
        let rc = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V2,
                (&mut info as *mut libc::rusage_info_v2).cast::<libc::rusage_info_t>(),
            )
        };
        if rc == 0 {
            info.ri_phys_footprint
        } else {
            0
        }
    }

    #[cfg(target_os = "linux")]
    fn children(pid: i32) -> Vec<i32> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return out;
        };
        for entry in dir.flatten() {
            let Some(p) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else {
                continue;
            };
            // Field 4 (ppid) follows the parenthesised command name.
            let ppid = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .and_then(|s| s.parse::<i32>().ok());
            if ppid == Some(pid) {
                out.push(p);
            }
        }
        out
    }

    #[cfg(target_os = "linux")]
    fn resident_bytes(pid: i32) -> u64 {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        std::fs::read_to_string(format!("/proc/{pid}/statm"))
            .ok()
            .and_then(|s| {
                s.split_whitespace()
                    .nth(1)
                    .and_then(|r| r.parse::<u64>().ok())
            })
            .map_or(0, |pages| pages * page)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::unix::task_cap;

    #[test]
    fn the_task_cap_leaves_room_but_is_finite() {
        assert_eq!(task_cap(1000, 1), 1000 + 256 + 256);
        assert_eq!(task_cap(0, 3), 256 + 768);
        assert_eq!(task_cap(u64::MAX, 3), u64::MAX);
    }
}
