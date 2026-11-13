//! Windows: AppContainer plus a Job Object (spec 9.2, T09).
//!
//! Windows has nothing like `sandbox-exec` or `bwrap` to wrap a command
//! with, so on Windows the runner does the sandboxing itself:
//!
//! - **AppContainer.** Each job gets its own container SID, derived from the
//!   job ID, and no capabilities: no network, loopback included. The job
//!   directory is the only place granted to that SID (full access, inherited
//!   by everything inside), so one job's worker can't open another's files.
//! - **Runtime.** The interpreter, libraries, adapters and engine installs
//!   must be readable by every AppContainer (`ALL APPLICATION PACKAGES`),
//!   as Program Files already is. [`prepare`] adds that once to directories
//!   that lack it: a per-user install or a development checkout.
//! - **Job Object.** It holds the worker and everything it starts:
//!   committed memory capped at the limit (the runner reports
//!   `E_RESOURCE_MEMORY` when it's hit), a process count, no clipboard,
//!   desktops, global atoms or system settings, no crash dialogs, and every
//!   process killed when the runner exits.
//! - **Start.** The worker is created suspended and is in the job before it
//!   runs anything; it inherits only the three standard handles.
//!
//! **Compile-checked only so far:** the probe (`self_test`) must pass on a
//! Windows machine or the Windows CI job before this is relied on.

use std::ffi::{c_void, OsStr};
use std::io;
use std::iter::repeat_n;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, ERROR_SUCCESS, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, EqualSid, FreeSid, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE,
    ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_CAPABILITIES, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectAssociateCompletionPortInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
};
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, JOB_OBJECT_MSG_JOB_MEMORY_LIMIT,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};

use crate::runner::{report_memory, Args, EX_UNAVAILABLE, KILLED_FOR_MEMORY};

/// `ALL APPLICATION PACKAGES`: every AppContainer.
const ALL_APP_PACKAGES: &str = "S-1-15-2-1";
const READ_EXECUTE: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
const POLL_MS: u32 = 50;
/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const PROFILE_EXISTS: i32 = 0x8007_00B7_u32 as i32;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", io::Error::last_os_error())
}

/// A SID and how to free it: `FreeSid` for derived ones, `LocalFree` for
/// parsed ones.
struct Sid {
    ptr: PSID,
    derived: bool,
}

impl Sid {
    fn parse(text: &str) -> Result<Sid, String> {
        let text = wide(OsStr::new(text));
        let mut ptr: PSID = null_mut();
        // SAFETY: `text` is NUL-terminated; on success `ptr` is ours to LocalFree.
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut ptr) } == 0 {
            return Err(last_error("parsing a SID"));
        }
        Ok(Sid {
            ptr,
            derived: false,
        })
    }

    /// The container SID for `name`. Deterministic: the same job always
    /// gets the same SID, and no two jobs share one.
    fn container(name: &str) -> Result<Sid, String> {
        let name = wide(OsStr::new(name));
        let mut ptr: PSID = null_mut();
        // SAFETY: `name` is NUL-terminated; on success `ptr` is ours to FreeSid.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut ptr) };
        if hr < 0 {
            return Err(format!("deriving the container SID: HRESULT {hr:#x}"));
        }
        Ok(Sid { ptr, derived: true })
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: freeing with the allocator that made it, once.
        unsafe {
            if self.derived {
                FreeSid(self.ptr);
            } else {
                LocalFree(self.ptr);
            }
        }
    }
}

/// Makes sure every AppContainer can read and run what's in `dirs` (the
/// worker runtime and engine installs). Directories already readable, like
/// Program Files, are left alone; missing ones are skipped.
pub(crate) fn prepare(dirs: &[&Path]) -> Result<(), String> {
    let all = Sid::parse(ALL_APP_PACKAGES)?;
    let mut failed = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        match grants(dir, &all, READ_EXECUTE) {
            Ok(true) => {}
            Ok(false) => {
                if let Err(e) = grant(dir, &all, READ_EXECUTE) {
                    failed.push(format!("{}: {e}", dir.display()));
                }
            }
            Err(e) => failed.push(format!("{}: {e}", dir.display())),
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "can't make the worker runtime readable: {}",
            failed.join("; ")
        ))
    }
}

/// Whether `path`'s own DACL already allows `sid` everything in `mask`.
fn grants(path: &Path, sid: &Sid, mask: u32) -> Result<bool, String> {
    let name = wide(path.as_os_str());
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: out-pointers are valid; `sd` owns `dacl` and is LocalFree'd below.
    let rc = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(format!("reading permissions: error {rc}"));
    }
    let found = if dacl.is_null() {
        true // no DACL at all: everyone has access
    } else {
        // SAFETY: `dacl` is a valid ACL inside `sd`; each ACE is read by its header.
        unsafe { acl_grants(dacl, sid.ptr, mask) }
    };
    // SAFETY: allocated by GetNamedSecurityInfoW.
    unsafe { LocalFree(sd) };
    Ok(found)
}

unsafe fn acl_grants(dacl: *const ACL, sid: PSID, mask: u32) -> bool {
    let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
    let ok = unsafe {
        GetAclInformation(
            dacl,
            (&raw mut info).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return false;
    }
    for i in 0..info.AceCount {
        let mut ace: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, i, &mut ace) } == 0 {
            continue;
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
        {
            continue;
        }
        let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        let ace_sid = (&raw const allowed.SidStart) as PSID;
        if unsafe { EqualSid(ace_sid, sid) } != 0 && allowed.Mask & mask == mask {
            return true;
        }
    }
    false
}

/// Adds an allow ACE for `sid` to `dir`, inherited by everything inside
/// (Windows applies it to what's already there).
fn grant(dir: &Path, sid: &Sid, mask: u32) -> Result<(), String> {
    let name = wide(dir.as_os_str());
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: as in `grants`.
    let rc = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(format!("reading permissions: error {rc}"));
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.ptr.cast(),
        },
    };
    let mut updated: *mut ACL = null_mut();
    // SAFETY: `dacl` (possibly null) stays valid until `sd` is freed after this.
    let rc = unsafe { SetEntriesInAclW(1, &entry, dacl, &mut updated) };
    // SAFETY: allocated by GetNamedSecurityInfoW; `dacl` isn't used again.
    unsafe { LocalFree(sd) };
    if rc != ERROR_SUCCESS {
        return Err(format!("building permissions: error {rc}"));
    }
    // SAFETY: `updated` is a valid ACL from SetEntriesInAclW, freed after.
    let rc = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            updated,
            null(),
        )
    };
    // SAFETY: allocated by SetEntriesInAclW.
    unsafe { LocalFree(updated.cast()) };
    if rc != ERROR_SUCCESS {
        return Err(format!("writing permissions: error {rc}"));
    }
    Ok(())
}

/// A container name for the job: `[-_. A-Za-z0-9]`, at most 64 characters.
fn container_name(job_id: &str) -> String {
    let start = job_id.len().saturating_sub(64 - "lfcs.".len());
    format!("lfcs.{}", &job_id[start..])
}

/// Quotes one argument the way `CommandLineToArgvW` and the C runtime
/// split them.
fn quote(arg: &str, out: &mut String) {
    if !out.is_empty() {
        out.push(' ');
    }
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(repeat_n('\\', backslashes * 2 + 1));
        } else {
            out.extend(repeat_n('\\', backslashes));
        }
        out.push(c);
        backslashes = 0;
    }
    out.extend(repeat_n('\\', backslashes * 2));
    out.push('"');
}

fn command_line(program: &[String]) -> String {
    let mut line = String::new();
    for arg in program {
        quote(arg, &mut line);
    }
    line
}

/// Owned handles, closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: our handle, closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// The job: limits, UI restrictions and a completion port for its messages.
fn job(memory_limit: u64, max_processes: u32) -> Result<(Owned, Owned), String> {
    // SAFETY: standard Job Object setup; every result is checked.
    unsafe {
        let job = Owned(CreateJobObjectW(null(), null()));
        if job.0.is_null() {
            return Err(last_error("creating the job"));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = max_processes.max(1);
        limits.JobMemoryLimit = usize::try_from(memory_limit).unwrap_or(usize::MAX);
        if SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Err(last_error("limiting the job"));
        }
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_EXITWINDOWS,
        };
        if SetInformationJobObject(
            job.0,
            JobObjectBasicUIRestrictions,
            (&raw const ui).cast(),
            size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        ) == 0
        {
            return Err(last_error("restricting the job"));
        }
        let port = Owned(CreateIoCompletionPort(
            INVALID_HANDLE_VALUE,
            null_mut(),
            0,
            1,
        ));
        if port.0.is_null() {
            return Err(last_error("creating the job's port"));
        }
        let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
            CompletionKey: job.0,
            CompletionPort: port.0,
        };
        if SetInformationJobObject(
            job.0,
            JobObjectAssociateCompletionPortInformation,
            (&raw const assoc).cast(),
            size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
        ) == 0
        {
            return Err(last_error("watching the job"));
        }
        Ok((job, port))
    }
}

/// Starts `program` suspended in the container, with only the standard
/// handles. Returns the process and its main thread.
fn spawn(program: &[String], sid: &Sid) -> Result<PROCESS_INFORMATION, String> {
    // SAFETY: every pointer passed lives until CreateProcessW returns; the
    // attribute list is initialised before use and deleted after.
    unsafe {
        let mut handles: Vec<HANDLE> = Vec::with_capacity(3);
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(which);
            if !h.is_null() && h != INVALID_HANDLE_VALUE && !handles.contains(&h) {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
                handles.push(h);
            }
        }
        let caps = SECURITY_CAPABILITIES {
            AppContainerSid: sid.ptr,
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size);
        // usize elements keep the list pointer-aligned.
        let mut buf = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list = buf.as_mut_ptr().cast::<c_void>();
        if InitializeProcThreadAttributeList(list, 2, 0, &mut size) == 0 {
            return Err(last_error("preparing the worker"));
        }
        let mut ok = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            (&raw const caps).cast(),
            size_of::<SECURITY_CAPABILITIES>(),
            null_mut(),
            null(),
        ) != 0;
        if ok && !handles.is_empty() {
            ok = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                handles.len() * size_of::<HANDLE>(),
                null_mut(),
                null(),
            ) != 0;
        }
        if !ok {
            let e = last_error("preparing the worker");
            DeleteProcThreadAttributeList(list);
            return Err(e);
        }
        let mut si: STARTUPINFOEXW = zeroed();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        if !handles.is_empty() {
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
            si.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
            si.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE);
        }
        si.lpAttributeList = list;
        let app = wide(OsStr::new(&program[0]));
        let mut line = wide(OsStr::new(&command_line(program)));
        let mut pi: PROCESS_INFORMATION = zeroed();
        let created = CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            i32::from(!handles.is_empty()),
            EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_NO_WINDOW,
            null(),
            null(), // the runner's directory: the job directory
            &si.StartupInfo,
            &mut pi,
        );
        let e = last_error("starting the worker");
        DeleteProcThreadAttributeList(list);
        if created == 0 {
            return Err(e);
        }
        Ok(pi)
    }
}

pub(crate) fn run(args: Args) -> ExitCode {
    match run_inner(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("sandbox runner: {e}");
            ExitCode::from(EX_UNAVAILABLE)
        }
    }
}

fn run_inner(args: &Args) -> Result<ExitCode, String> {
    let job_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let name = container_name(&args.job_id);
    let sid = Sid::container(&name)?;
    grant(&job_dir, &sid, FILE_ALL_ACCESS)?;
    let (job, port) = job(args.memory_limit, args.max_processes)?;
    // No error dialogs from the worker or its engines (inherited).
    // SAFETY: plain call.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)
    };

    // Starting in a container normally needs only its SID; a registered
    // profile is the fallback, deleted again when the worker is done.
    let mut profile = false;
    let pi = match spawn(&args.program, &sid) {
        Ok(pi) => pi,
        Err(first) => {
            let wname = wide(OsStr::new(&name));
            let mut psid: PSID = null_mut();
            // SAFETY: valid strings; the returned SID is freed below.
            let hr = unsafe {
                CreateAppContainerProfile(
                    wname.as_ptr(),
                    wname.as_ptr(),
                    wname.as_ptr(),
                    null(),
                    0,
                    &mut psid,
                )
            };
            if hr < 0 && hr != PROFILE_EXISTS {
                return Err(first);
            }
            if !psid.is_null() {
                // SAFETY: allocated by CreateAppContainerProfile.
                unsafe { FreeSid(psid) };
            }
            profile = hr >= 0;
            spawn(&args.program, &sid)?
        }
    };
    let process = Owned(pi.hProcess);
    let thread = Owned(pi.hThread);
    // SAFETY: handles from CreateProcessW; the process is still suspended.
    unsafe {
        if AssignProcessToJobObject(job.0, process.0) == 0 {
            let e = last_error("confining the worker");
            TerminateProcess(process.0, 1);
            return Err(e);
        }
        if ResumeThread(thread.0) == u32::MAX {
            let e = last_error("resuming the worker");
            TerminateJobObject(job.0, 1);
            return Err(e);
        }
    }
    drop(thread);

    let code = loop {
        if memory_hit(&port, POLL_MS) {
            // SAFETY: our job and process handles.
            unsafe {
                TerminateJobObject(job.0, u32::from(KILLED_FOR_MEMORY));
                WaitForSingleObject(process.0, 5000);
            }
            report_memory(&args.job_id, args.memory_limit);
            break ExitCode::from(KILLED_FOR_MEMORY);
        }
        // SAFETY: our process handle.
        match unsafe { WaitForSingleObject(process.0, 0) } {
            WAIT_TIMEOUT => {}
            WAIT_OBJECT_0 => {
                // An allocation that failed just before exit still counts.
                if memory_hit(&port, 0) {
                    report_memory(&args.job_id, args.memory_limit);
                    break ExitCode::from(KILLED_FOR_MEMORY);
                }
                let mut status = 0u32;
                // SAFETY: our process handle, which has exited.
                unsafe { GetExitCodeProcess(process.0, &mut status) };
                break ExitCode::from(u8::try_from(status).unwrap_or(u8::MAX));
            }
            _ => {
                // SAFETY: our job handle.
                unsafe { TerminateJobObject(job.0, 1) };
                return Err(last_error("waiting for the worker"));
            }
        }
    };
    // Closing the job kills anything the worker left running.
    drop(job);
    if profile {
        let wname = wide(OsStr::new(&name));
        // SAFETY: valid string.
        unsafe { DeleteAppContainerProfile(wname.as_ptr()) };
    }
    Ok(code)
}

/// Whether the job reported hitting its memory limit within `wait_ms`.
fn memory_hit(port: &Owned, wait_ms: u32) -> bool {
    let mut wait = wait_ms;
    loop {
        let mut message = 0u32;
        let mut key = 0usize;
        let mut overlapped: *mut OVERLAPPED = null_mut();
        // SAFETY: our port; out-pointers are valid.
        let got = unsafe {
            GetQueuedCompletionStatus(port.0, &mut message, &mut key, &mut overlapped, wait)
        };
        if got == 0 {
            return false;
        }
        if message == JOB_OBJECT_MSG_JOB_MEMORY_LIMIT {
            return true;
        }
        wait = 0; // drain what's queued, then return
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_round_trip_through_the_command_line() {
        let args: Vec<String> = [
            r"C:\Program Files\Python\python.exe",
            "-I",
            r#"{"home":"C:\\Users\\a b","n":1}"#,
            "",
            r"ends\",
            r#"a\"b"#,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(
            command_line(&args),
            r#""C:\Program Files\Python\python.exe" -I "{\"home\":\"C:\\Users\\a b\",\"n\":1}" "" ends\ "a\\\"b""#
        );
    }

    #[test]
    fn container_names_fit() {
        assert_eq!(container_name("j_probe"), "lfcs.j_probe");
        assert!(container_name(&"x".repeat(80)).len() <= 64);
    }
}
