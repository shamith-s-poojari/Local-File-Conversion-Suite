use std::fmt;
use std::io;

use crate::id::JobId;
use crate::store::JobState;

/// Errors from the job store. Messages never include paths, file names or
/// document content, so they're safe to log.
#[derive(Debug)]
pub enum JobError {
    UnknownJob(JobId),
    /// The section 10.1 state machine doesn't allow this change.
    InvalidTransition {
        from: JobState,
        to: JobState,
    },
    /// The operation needs the job in a different state.
    WrongState {
        needed: &'static str,
        actual: JobState,
    },
    /// Another save or discard of this job is in progress.
    Busy,
    /// Not enough free space for staging (`E_RESOURCE_DISK`).
    InsufficientDisk {
        required: u64,
        available: u64,
    },
    /// A caller passed something unusable, such as a source that isn't a
    /// regular file.
    InvalidInput(&'static str),
    /// A recorded output isn't a regular file inside `out/` (`E_PROTOCOL`).
    InvalidOutput(&'static str),
    /// The save destination is one of the job's inputs, and the user hasn't
    /// confirmed overwriting it (spec section 10.6).
    NeedsInputOverwriteConfirmation,
    /// The data root became unusable.
    DataRoot(crate::DataRootError),
    Io {
        context: &'static str,
        source: io::Error,
    },
}

impl JobError {
    pub(crate) fn io(context: &'static str) -> impl FnOnce(io::Error) -> JobError {
        move |source| JobError::Io { context, source }
    }

    /// The catalog code (spec section 13.1) to report when this error fails
    /// a job, if it should fail one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            JobError::InsufficientDisk { .. } => Some("E_RESOURCE_DISK"),
            JobError::InvalidOutput(_) => Some("E_PROTOCOL"),
            JobError::Io { source, .. } if is_out_of_space(source) => Some("E_RESOURCE_DISK"),
            JobError::Io { .. } | JobError::DataRoot(_) => Some("E_INTERNAL"),
            _ => None,
        }
    }
}

fn is_out_of_space(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::StorageFull {
        return true;
    }
    // ERROR_DISK_FULL and ERROR_HANDLE_DISK_FULL on Windows.
    cfg!(windows) && matches!(e.raw_os_error(), Some(112 | 39))
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobError::UnknownJob(id) => write!(f, "unknown job {id}"),
            JobError::InvalidTransition { from, to } => {
                write!(f, "a job can't go from {from:?} to {to:?}")
            }
            JobError::WrongState { needed, actual } => {
                write!(f, "the job must be {needed}, but it is {actual:?}")
            }
            JobError::Busy => f.write_str("the job is being saved or discarded"),
            JobError::InsufficientDisk {
                required,
                available,
            } => write!(
                f,
                "not enough free space: {required} bytes needed, {available} available"
            ),
            JobError::InvalidInput(why) => write!(f, "invalid input: {why}"),
            JobError::InvalidOutput(why) => write!(f, "invalid output: {why}"),
            JobError::NeedsInputOverwriteConfirmation => {
                f.write_str("saving here would overwrite one of the job's input files")
            }
            JobError::DataRoot(e) => e.fmt(f),
            JobError::Io { context, source } => write!(f, "{context}: {source}"),
        }
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            JobError::Io { source, .. } => Some(source),
            JobError::DataRoot(e) => Some(e),
            _ => None,
        }
    }
}
