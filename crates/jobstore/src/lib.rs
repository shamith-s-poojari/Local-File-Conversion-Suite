//! Job store, job directories, staging, garbage collection and saving
//! (spec section 10, blueprint sections 5.4 and 6.5).
//!
//! - [`JobStore`] holds every job in memory (no job history on disk), checks
//!   each state change against the section 10.1 state machine, and numbers
//!   every change with a per-app monotonically increasing `seq`.
//! - [`DataRoot`] resolves `<dataRoot>/jobs` and refuses a data root inside a
//!   cloud-sync folder.
//! - [`SyncRoots`] detects OneDrive, Dropbox, iCloud Drive and Google Drive
//!   folders. Detection is best effort.
//! - [`Lease`] records the owning process (PID and start time) so that a
//!   crashed run's job directories can be collected at the next start.
//! - [`sanitize_file_name`] makes a worker's `suggestedName` safe to save.
//!
//! Nothing here writes file names, paths or document content into error
//! messages, so errors can be logged as they are.

mod data_root;
mod error;
mod fsutil;
mod id;
mod lease;
mod name;
mod save;
mod store;
mod sync;

pub use data_root::{default_data_root, DataRoot, DataRootError, APP_DIR_NAME};
pub use error::JobError;
pub use fsutil::RetryPolicy;
pub use id::JobId;
pub use lease::Lease;
pub use name::sanitize_file_name;
pub use save::{SaveCheck, SaveDestination, SaveOptions, SaveReport};
pub use store::{
    InputSource, JobDirs, JobEvent, JobFailure, JobSnapshot, JobState, JobStore, OutputFile,
    Progress, ShutdownReport, Snapshot, StoreConfig, SweepReport, Warning,
};
pub use sync::{SyncEnv, SyncOs, SyncProvider, SyncRoots};
