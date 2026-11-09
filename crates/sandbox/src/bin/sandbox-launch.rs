//! `sandbox-launch <spec-json>`: applies the sandbox and runs a worker.
//!
//! Started only by the core's supervisor. Exits with the worker's exit code,
//! or with [`SANDBOX_SETUP_FAILED_EXIT`] if the sandbox couldn't be applied,
//! in which case the worker never started. The reason goes to stderr, which
//! the supervisor keeps in memory only.

use converter_sandbox::{SandboxSpec, SANDBOX_SETUP_FAILED_EXIT};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(arg), None) = (args.next(), args.next()) else {
        eprintln!("usage: sandbox-launch <spec-json>");
        std::process::exit(SANDBOX_SETUP_FAILED_EXIT);
    };
    let spec = match SandboxSpec::from_arg(&arg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sandbox-launch: {e}");
            std::process::exit(SANDBOX_SETUP_FAILED_EXIT);
        }
    };
    std::process::exit(launch(&spec));
}

#[cfg(windows)]
fn launch(spec: &SandboxSpec) -> i32 {
    match converter_sandbox::windows::run(spec) {
        Ok(code) => code as i32,
        Err(e) => {
            eprintln!("sandbox-launch: {e}");
            SANDBOX_SETUP_FAILED_EXIT
        }
    }
}

#[cfg(not(windows))]
fn launch(_: &SandboxSpec) -> i32 {
    eprintln!("sandbox-launch: no sandbox for this OS yet (T10)");
    SANDBOX_SETUP_FAILED_EXIT
}
