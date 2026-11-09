//! Killing a worker's whole process tree (spec section 10.3).
//!
//! - Unix: the worker starts in its own process group, and the group is sent
//!   `SIGKILL`.
//! - Windows: the worker is put in a Job Object with "kill on close", and the
//!   job is terminated. If the core itself dies, the OS closes the handle and
//!   the tree dies with it.
//!
//! Known gap on Windows: a child the worker starts before it's assigned to
//! the job escapes it. Workers don't start children that early; the sandbox
//! (T09) creates its processes suspended, which closes the gap.

use std::io;
use std::process::{Child, Command};

/// Configures a command so its tree can be killed later.
pub(crate) fn prepare(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
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

    /// Kills every process in the tree. Safe to call more than once.
    pub(crate) fn kill(&self) {
        #[cfg(unix)]
        // SAFETY: plain syscall; a negative PID addresses the process group.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
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
