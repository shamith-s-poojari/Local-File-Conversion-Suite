//! The worker runner: starts a sandboxed worker and enforces its memory limit.
//!
//! ```text
//! <runner> [prefix args] --memory-limit BYTES --job-id ID -- PROGRAM [ARGS...]
//! ```
//!
//! The worker inherits stdin, stdout and stderr, so the protocol flows
//! straight between it and the core. Every 50 ms the runner sums the
//! resident memory of the worker and its descendants (an RSS watchdog: spec
//! 9.2, since `RLIMIT_AS` is unreliable on macOS). Over the limit, it kills
//! them and writes a protocol `error` with `E_RESOURCE_MEMORY`, which the
//! scheduler treats like any worker error (retry once, alone, larger).
//!
//! The runner stays in the scheduler's process group, so killing the group
//! kills it and the worker together. Exit status mirrors the worker's.
//!
//! It runs as `converter-sandbox-run`, or as the desktop app itself started
//! with `--sandbox-run` (so releases ship no extra binary).

use std::process::ExitCode;

/// Runs the runner with these arguments (without the program name or any
/// prefix such as `--sandbox-run`).
#[cfg(unix)]
pub fn main_with(args: Vec<String>) -> ExitCode {
    unix::main(args)
}

#[cfg(not(unix))]
pub fn main_with(_: Vec<String>) -> ExitCode {
    eprintln!("sandbox runner: no sandbox on this platform yet");
    ExitCode::from(70)
}

#[cfg(unix)]
mod unix {
    use std::io::Write;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Command, ExitCode};
    use std::thread;
    use std::time::Duration;

    const POLL: Duration = Duration::from_millis(50);
    const EX_USAGE: u8 = 64;
    const EX_UNAVAILABLE: u8 = 69;
    const KILLED_FOR_MEMORY: u8 = 137;

    struct Args {
        memory_limit: u64,
        job_id: String,
        program: Vec<String>,
    }

    fn parse(args: Vec<String>) -> Option<Args> {
        let mut args = args.into_iter();
        let mut memory_limit = None;
        let mut job_id = None;
        while let Some(a) = args.next() {
            match a.as_str() {
                "--memory-limit" => memory_limit = args.next()?.parse().ok(),
                "--job-id" => job_id = args.next(),
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
            program,
        })
    }

    pub fn main(args: Vec<String>) -> ExitCode {
        let Some(args) = parse(args) else {
            eprintln!("usage: converter-sandbox-run --memory-limit N --job-id ID -- PROGRAM ...");
            return ExitCode::from(EX_USAGE);
        };
        let mut cmd = Command::new(&args.program[0]);
        cmd.args(&args.program[1..]);
        let limit = args.memory_limit;
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

    fn report_memory(job_id: &str, limit: u64) {
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

    #[cfg(target_os = "macos")]
    fn resident_bytes(pid: i32) -> u64 {
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: `info` is valid for `size` bytes.
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            )
        };
        if n == size {
            info.pti_resident_size
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
