//! Windows sandbox: AppContainer plus a Job Object (spec 9.2, C1).
//!
//! What [`run`] enforces on the worker:
//!
//! - **Identity:** an AppContainer token whose SID is derived from the
//!   per-job container name. AppContainer tokens fail every access check
//!   unless the object grants the container SID, one of its capabilities, or
//!   `ALL APPLICATION PACKAGES`. The user's files grant none of those.
//! - **Filesystem:** the container SID gets full access to the job
//!   directory, and nothing else. The runtime (Python, packages, adapters) is
//!   readable through the [`RUNTIME_CAPABILITY`](crate::RUNTIME_CAPABILITY)
//!   capability, granted once by [`grant_runtime`].
//! - **Network:** no network capabilities, which also blocks loopback.
//! - **Processes:** a Job Object with a memory limit, an active-process
//!   limit, and kill-on-close. The worker is placed in the job atomically at
//!   creation (`PROC_THREAD_ATTRIBUTE_JOB_LIST`) and created suspended, so
//!   there's no window where it runs outside the job. With a process limit
//!   of 1 the child-process policy also forbids creating children at all.
//! - **Handles:** only the three stdio handles are inherited
//!   (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`).
//! - **Environment:** exactly the spec's variables; nothing is inherited.
//!
//! If the shim dies (the supervisor terminates its Job Object), its handle
//! to the worker's job closes and kill-on-close ends the worker.

use std::ffi::{c_void, OsStr};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
    TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, DeriveCapabilitySidsFromName, EqualSid, FreeSid, GetAce, GetAclInformation,
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, SE_GROUP_ENABLED};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS,
    EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;

use crate::{SandboxSpec, RUNTIME_CAPABILITY};

const READ_EXECUTE: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn win32(code: u32, what: &str) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{what}: {}",
            io::Error::from_raw_os_error(code as i32)
        )))
    }
}

fn check(ok: i32, what: &str) -> io::Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        let e = io::Error::last_os_error();
        Err(io::Error::other(format!("{what}: {e}")))
    }
}

/// An AppContainer SID, freed with `FreeSid`.
struct ContainerSid(PSID);

impl ContainerSid {
    fn derive(name: &str) -> io::Result<Self> {
        let mut sid: PSID = null_mut();
        let name = wide(name);
        // SAFETY: valid NUL-terminated name and out pointer.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
        if hr < 0 || sid.is_null() {
            return Err(io::Error::other(format!(
                "DeriveAppContainerSidFromAppContainerName: 0x{hr:08x}"
            )));
        }
        Ok(ContainerSid(sid))
    }
}

impl Drop for ContainerSid {
    fn drop(&mut self) {
        // SAFETY: allocated by DeriveAppContainerSidFromAppContainerName.
        unsafe { FreeSid(self.0) };
    }
}

/// A registered AppContainer profile, deleted on drop.
///
/// Windows won't start a process in an AppContainer without its profile
/// (the container's object directory comes from it). Profiles left behind
/// by a killed shim are removed by [`remove_stale_profiles`] at app start.
struct Profile {
    name: Vec<u16>,
}

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const HR_ALREADY_EXISTS: i32 = 0x8007_00B7_u32 as i32;

impl Profile {
    fn create(name: &str) -> io::Result<Self> {
        let wname = wide(name);
        let mut sid: PSID = null_mut();
        // SAFETY: valid strings; no capabilities; sid is freed below.
        let hr = unsafe {
            CreateAppContainerProfile(
                wname.as_ptr(),
                wname.as_ptr(),
                wname.as_ptr(),
                null(),
                0,
                &mut sid,
            )
        };
        if hr >= 0 && !sid.is_null() {
            // SAFETY: allocated by CreateAppContainerProfile.
            unsafe { FreeSid(sid) };
        } else if hr != HR_ALREADY_EXISTS {
            return Err(io::Error::other(format!(
                "CreateAppContainerProfile: 0x{hr:08x}"
            )));
        }
        Ok(Profile { name: wname })
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        // SAFETY: valid name. Best effort.
        unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
    }
}

/// Deletes AppContainer profiles whose name starts with `prefix`. The app
/// calls it at start with [`PROFILE_PREFIX`](crate::PROFILE_PREFIX), after
/// any previous run's workers are gone. Returns how many it removed.
pub fn remove_stale_profiles(prefix: &str) -> usize {
    const MAPPINGS: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Mappings";
    let mut names = Vec::new();
    // SAFETY: registry reads with checked results; the key is closed below.
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(MAPPINGS).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ) != 0
        {
            return 0;
        }
        let moniker = wide("Moniker");
        for i in 0.. {
            let mut sid = [0u16; 256];
            let mut len = sid.len() as u32;
            let err = RegEnumKeyExW(
                key,
                i,
                sid.as_mut_ptr(),
                &mut len,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            );
            if err != 0 {
                break;
            }
            let mut value = [0u16; 128];
            let mut bytes = size_of_val(&value) as u32;
            let err = RegGetValueW(
                key,
                sid.as_ptr(),
                moniker.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                value.as_mut_ptr().cast(),
                &mut bytes,
            );
            if err != 0 {
                continue;
            }
            let end = value.iter().position(|&c| c == 0).unwrap_or(value.len());
            let name = String::from_utf16_lossy(&value[..end]);
            if name.starts_with(prefix) {
                names.push(name);
            }
        }
        RegCloseKey(key);
    }
    names
        .iter()
        // SAFETY: valid name.
        .filter(|n| unsafe { DeleteAppContainerProfile(wide(n).as_ptr()) } >= 0)
        .count()
}

/// The runtime capability SID, freed with `LocalFree`.
struct CapabilitySid {
    sid: PSID,
    groups: *mut PSID,
    group_count: u32,
    caps: *mut PSID,
}

impl CapabilitySid {
    fn derive(name: &str) -> io::Result<Self> {
        let name = wide(name);
        let (mut groups, mut group_count, mut caps, mut cap_count) =
            (null_mut(), 0u32, null_mut(), 0u32);
        // SAFETY: valid name and out pointers.
        let ok = unsafe {
            DeriveCapabilitySidsFromName(
                name.as_ptr(),
                &mut groups,
                &mut group_count,
                &mut caps,
                &mut cap_count,
            )
        };
        check(ok, "DeriveCapabilitySidsFromName")?;
        let mut me = CapabilitySid {
            sid: null_mut(),
            groups,
            group_count,
            caps,
        };
        if cap_count != 1 || caps.is_null() {
            return Err(io::Error::other("expected one capability SID"));
        }
        // SAFETY: caps holds cap_count SIDs.
        me.sid = unsafe { *caps };
        Ok(me)
    }
}

impl Drop for CapabilitySid {
    fn drop(&mut self) {
        // SAFETY: each SID and both arrays are LocalAlloc'd by the API.
        unsafe {
            for i in 0..self.group_count as usize {
                LocalFree(*self.groups.add(i));
            }
            LocalFree(self.groups.cast());
            if !self.sid.is_null() {
                LocalFree(self.sid);
            }
            LocalFree(self.caps.cast());
        }
    }
}

/// True if `dacl` already has an inheritable allow ACE for `sid` covering
/// `mask`.
fn has_grant(dacl: *const ACL, sid: PSID, mask: u32) -> bool {
    if dacl.is_null() {
        return false;
    }
    // SAFETY: dacl is a valid ACL from GetNamedSecurityInfoW; GetAce returns
    // pointers into it.
    unsafe {
        let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
        if GetAclInformation(
            dacl,
            (&raw mut info).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
        {
            return false;
        }
        for i in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                continue;
            }
            let header = &*(ace as *const ACE_HEADER);
            if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
                continue;
            }
            let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
            let ace_sid = (&raw const allowed.SidStart).cast_mut().cast();
            let inherits = u32::from(header.AceFlags) & SUB_CONTAINERS_AND_OBJECTS_INHERIT
                == SUB_CONTAINERS_AND_OBJECTS_INHERIT;
            if inherits && allowed.Mask & mask == mask && EqualSid(ace_sid, sid) != 0 {
                return true;
            }
        }
    }
    false
}

/// Adds an inheritable allow ACE for `sid` to `path`, propagating to
/// everything below it. Does nothing if an equivalent ACE is already there.
fn grant(path: &Path, sid: PSID, mask: u32) -> io::Result<()> {
    let name = wide(path);
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: valid name and out pointers; sd is freed below.
    let err = unsafe {
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
    win32(err, &format!("reading the ACL of {}", path.display()))?;
    let result = (|| {
        if has_grant(dacl, sid, mask) {
            return Ok(());
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
                ptstrName: sid.cast(),
            },
        };
        let mut new_dacl: *mut ACL = null_mut();
        // SAFETY: one valid entry; new_dacl is freed below.
        let err = unsafe { SetEntriesInAclW(1, &entry, dacl, &mut new_dacl) };
        win32(err, "SetEntriesInAclW")?;
        // SAFETY: valid name and ACL. This also applies the inheritable ACE
        // to everything already below `path`.
        let err = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                new_dacl,
                null(),
            )
        };
        // SAFETY: allocated by SetEntriesInAclW.
        unsafe { LocalFree(new_dacl.cast()) };
        win32(err, &format!("setting the ACL of {}", path.display()))
    })();
    // SAFETY: allocated by GetNamedSecurityInfoW.
    unsafe { LocalFree(sd) };
    result
}

/// Grants the runtime capability read and execute access to `dirs`.
/// Idempotent and cheap once done; the first call walks each tree.
pub fn grant_runtime(dirs: &[impl AsRef<Path>]) -> Result<(), String> {
    let cap = CapabilitySid::derive(RUNTIME_CAPABILITY).map_err(|e| e.to_string())?;
    for dir in dirs {
        grant(dir.as_ref(), cap.sid, READ_EXECUTE).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Owned Win32 handle.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn job_object(spec: &SandboxSpec) -> io::Result<Handle> {
    // SAFETY: plain Job Object setup with checked results.
    unsafe {
        let job = Handle(CreateJobObjectW(null(), null()));
        if job.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_JOB_MEMORY;
        info.BasicLimitInformation.ActiveProcessLimit = spec.process_limit.max(1);
        info.JobMemoryLimit = usize::try_from(spec.memory_limit).unwrap_or(usize::MAX);
        check(
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ),
            "SetInformationJobObject",
        )?;
        Ok(job)
    }
}

/// Quotes one argument the way `CommandLineToArgvW` and the MSVC runtime
/// parse it.
fn quote_arg(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

fn command_line(spec: &SandboxSpec) -> String {
    let mut line = String::new();
    quote_arg(&spec.program.to_string_lossy(), &mut line);
    for a in &spec.args {
        line.push(' ');
        quote_arg(a, &mut line);
    }
    line
}

fn environment_block(spec: &SandboxSpec) -> Vec<u16> {
    let mut vars = spec.env.clone();
    // Windows expects the block sorted, case-insensitively.
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(OsStr::new(&format!("{k}={v}")).encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// Our stdio handles, made inheritable, without duplicates or invalid ones.
fn stdio_handles() -> io::Result<[HANDLE; 3]> {
    let mut out = [null_mut(); 3];
    for (slot, which) in out
        .iter_mut()
        .zip([STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE])
    {
        // SAFETY: plain query.
        let h = unsafe { GetStdHandle(which) };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(io::Error::other("the shim needs stdin, stdout and stderr"));
        }
        // SAFETY: valid handle owned by this process.
        check(
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
            "SetHandleInformation",
        )?;
        *slot = h;
    }
    Ok(out)
}

/// A thread attribute list with storage for its values.
struct Attributes {
    buf: Vec<usize>,
}

impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: size query with a null list is the documented pattern.
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) };
        let mut buf = vec![0usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: buf has at least `size` bytes, pointer-aligned.
        check(
            unsafe {
                InitializeProcThreadAttributeList(buf.as_mut_ptr().cast(), count, 0, &mut size)
            },
            "InitializeProcThreadAttributeList",
        )?;
        Ok(Attributes { buf })
    }

    fn list(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buf.as_mut_ptr().cast()
    }

    /// # Safety
    /// `value` must stay valid until the list is dropped.
    unsafe fn set<T>(&mut self, attr: u32, value: *const T, size: usize) -> io::Result<()> {
        check(
            UpdateProcThreadAttribute(
                self.list(),
                0,
                attr as usize,
                value.cast(),
                size,
                null_mut(),
                null(),
            ),
            "UpdateProcThreadAttribute",
        )
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized in new().
        unsafe { DeleteProcThreadAttributeList(self.list()) };
    }
}

/// Applies the sandbox, runs the worker to completion and returns its exit
/// code. An error means the sandbox couldn't be applied and the worker
/// never ran.
pub fn run(spec: &SandboxSpec) -> io::Result<u32> {
    if !spec.program.is_absolute() {
        return Err(io::Error::other("program must be an absolute path"));
    }
    // Declared first, so it's dropped last: after the worker and its job.
    let _profile = Profile::create(&spec.container)?;
    let container = ContainerSid::derive(&spec.container)?;
    let capability = CapabilitySid::derive(RUNTIME_CAPABILITY)?;
    for dir in &spec.read_write {
        grant(dir, container.0, FILE_ALL_ACCESS)?;
    }
    let job = job_object(spec)?;
    let handles = stdio_handles()?;

    let mut caps = [SID_AND_ATTRIBUTES {
        Sid: capability.sid,
        Attributes: SE_GROUP_ENABLED as u32,
    }];
    let security = SECURITY_CAPABILITIES {
        AppContainerSid: container.0,
        Capabilities: caps.as_mut_ptr(),
        CapabilityCount: caps.len() as u32,
        Reserved: 0,
    };
    let mut unique: Vec<HANDLE> = Vec::with_capacity(3);
    for h in handles {
        if !unique.contains(&h) {
            unique.push(h);
        }
    }
    let jobs = [job.0];
    let child_policy: u32 = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;

    let mut attrs = Attributes::new(4)?;
    // SAFETY: every value outlives `attrs`, which is dropped at the end of
    // this function after CreateProcessW.
    unsafe {
        attrs.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            &raw const security,
            size_of::<SECURITY_CAPABILITIES>(),
        )?;
        attrs.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            unique.as_ptr(),
            unique.len() * size_of::<HANDLE>(),
        )?;
        attrs.set(
            PROC_THREAD_ATTRIBUTE_JOB_LIST,
            jobs.as_ptr(),
            size_of_val(&jobs),
        )?;
        if spec.process_limit <= 1 {
            attrs.set(
                PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
                &raw const child_policy,
                size_of::<u32>(),
            )?;
        }
    }

    // SAFETY: zeroed is a valid initial state for STARTUPINFOEXW.
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = handles[0];
    si.StartupInfo.hStdOutput = handles[1];
    si.StartupInfo.hStdError = handles[2];
    si.lpAttributeList = attrs.list();

    let program = wide(&spec.program);
    let mut cmdline = wide(command_line(spec));
    let env = environment_block(spec);
    let cwd = wide(&spec.cwd);
    // SAFETY: zeroed is valid for an out struct.
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: all pointers are valid for the call; the handle list limits
    // inheritance to the stdio handles.
    let ok = unsafe {
        CreateProcessW(
            program.as_ptr(),
            cmdline.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | DETACHED_PROCESS
                | CREATE_SUSPENDED,
            env.as_ptr().cast(),
            cwd.as_ptr(),
            &si.StartupInfo,
            &mut pi,
        )
    };
    check(ok, "CreateProcessW")?;
    let process = Handle(pi.hProcess);
    let thread = Handle(pi.hThread);
    drop(attrs);

    // SAFETY: valid handles from CreateProcessW.
    unsafe {
        if ResumeThread(thread.0) == u32::MAX {
            let e = io::Error::last_os_error();
            // Dropping `job` kills the suspended worker.
            return Err(io::Error::other(format!("ResumeThread: {e}")));
        }
        drop(thread);
        if WaitForSingleObject(process.0, INFINITE) != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0u32;
        check(
            GetExitCodeProcess(process.0, &mut code),
            "GetExitCodeProcess",
        )?;
        // `job` drops here: anything left in it dies.
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(a: &str) -> String {
        let mut s = String::new();
        quote_arg(a, &mut s);
        s
    }

    #[test]
    fn quoting_follows_msvc_rules() {
        assert_eq!(q("plain"), "plain");
        assert_eq!(q(""), "\"\"");
        assert_eq!(q("a b"), "\"a b\"");
        assert_eq!(q("C:\\dir with space\\"), "\"C:\\dir with space\\\\\"");
        assert_eq!(q("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(q("a\\\"b"), "\"a\\\\\\\"b\"");
        assert_eq!(q("C:\\no\\space"), "C:\\no\\space");
    }

    #[test]
    fn environment_block_is_sorted_and_double_terminated() {
        let spec = SandboxSpec {
            container: String::new(),
            program: "C:\\p.exe".into(),
            args: vec![],
            cwd: "C:\\".into(),
            read_write: vec![],
            env: vec![("b".into(), "2".into()), ("A".into(), "1".into())],
            memory_limit: 0,
            process_limit: 1,
        };
        let block = String::from_utf16(&environment_block(&spec)).unwrap();
        assert_eq!(block, "A=1\0b=2\0\0");
    }
}
