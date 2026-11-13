//! Scheduler, watchdog and supervisor (spec sections 7.1, 7.4, 10.2 and
//! 10.3).
//!
//! - [`Scheduler`] admits queued jobs against the memory budget and the
//!   per-pool limits. It's pure, so its rules are tested without processes.
//! - [`Worker`] launches one worker through a [`Launcher`] (the sandbox),
//!   checks the `hello`, and supervises each request: line limits, schema,
//!   job ID, message rate, the watchdog, cancel, and killing the whole
//!   process tree.
//! - [`verify_outputs`] re-checks every output a worker claims.
//! - [`Runner`] drives a staged job through validate, estimate, admission,
//!   execute and verification, recording every step in the job store.

mod budget;
mod queue;
mod runner;
mod tree;
mod verify;
mod worker;

pub use budget::{budget, FixedMemory, MemoryProbe, SystemMemory};
pub use queue::{
    hard_limit, phase1_pools, Admission, Scheduler, Ticket, MIN_HARD_LIMIT, STARVATION_AFTER,
};
pub use runner::{
    pool_for, warning_catalog, JobPart, JobSpec, Runner, RunnerConfig, GENERIC_WARNING_CODE,
    MAX_DETAIL_BYTES,
};
pub use verify::verify_outputs;
pub use worker::{
    retryable, CancelToken, LaunchSpec, Launcher, RequestKind, Timing, Watchdog, Worker,
    WorkerEvent, MAX_MESSAGES_PER_SECOND, STDERR_BUFFER_BYTES,
};
