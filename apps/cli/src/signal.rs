//! Ctrl-C: the first one asks the running job to stop so it can be cleaned up
//! (audit R17); a second one exits at once.
//!
//! The handler only sets a flag. The command loops look at [`interrupted`]
//! between steps, cancel the job and let the session's `Drop` delete the job
//! folders. No new dependency: the two C functions needed are declared here;
//! the C library and kernel32 are already linked by the standard library.

use std::sync::atomic::{AtomicBool, Ordering};

/// Exit code after an interrupt: 128 + SIGINT, as shells report it.
pub const EXIT_INTERRUPTED: u8 = 130;

static HIT: AtomicBool = AtomicBool::new(false);

/// Whether Ctrl-C (or, on Unix, SIGTERM) arrived.
pub fn interrupted() -> bool {
    HIT.load(Ordering::SeqCst)
}

/// A second signal means the clean stop is taking too long (or is stuck in
/// something that doesn't look at the flag): leave now. Workers die with
/// their sandbox, and leftover job folders are removed at the next start.
fn on_signal() -> bool {
    HIT.swap(true, Ordering::SeqCst)
}

#[cfg(unix)]
pub fn install() {
    use std::os::raw::c_int;

    extern "C" {
        fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
        fn _exit(status: c_int) -> !;
    }
    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;

    extern "C" fn handler(_: c_int) {
        // Only an atomic swap and `_exit`: both are safe in a handler.
        if on_signal() {
            unsafe { _exit(EXIT_INTERRUPTED as c_int) }
        }
    }
    unsafe {
        signal(SIGINT, handler);
        signal(SIGTERM, handler);
    }
}

#[cfg(windows)]
pub fn install() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;

    unsafe extern "system" fn handler(kind: u32) -> i32 {
        // Closing the console or logging off: let Windows do its default.
        if kind != CTRL_C_EVENT && kind != CTRL_BREAK_EVENT {
            return 0;
        }
        if on_signal() {
            std::process::exit(EXIT_INTERRUPTED as i32);
        }
        1
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(any(unix, windows)))]
pub fn install() {}
