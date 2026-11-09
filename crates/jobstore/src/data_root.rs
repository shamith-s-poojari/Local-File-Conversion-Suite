//! The data root (spec section 10.4).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::fsutil::{ensure_private_dir, normalize};
use crate::sync::{SyncProvider, SyncRoots};

/// The app's folder name under the OS data directory: the bundle identifier
/// from `tauri.conf.json`, which is also what Tauri uses.
pub const APP_DIR_NAME: &str = "io.github.shamith-s-poojari.local-file-conversion-suite";

/// The default data root for this OS:
///
/// - Windows: `%LOCALAPPDATA%\<App>`
/// - macOS: `~/Library/Application Support/<App>`
/// - Linux: `$XDG_CACHE_HOME/<App>` (default `~/.cache/<App>`)
pub fn default_data_root() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        dirs::data_local_dir()
    } else if cfg!(target_os = "macos") {
        dirs::data_dir()
    } else {
        dirs::cache_dir()
    };
    base.map(|b| b.join(APP_DIR_NAME))
}

/// Why a data root can't be used.
#[derive(Debug)]
pub enum DataRootError {
    /// The data root is inside a cloud-sync folder, so job files would be
    /// uploaded. The app refuses to process and tells the user how to change
    /// the location.
    InSyncFolder(SyncProvider),
    /// No default location could be determined.
    NoDefault,
    Io(io::Error),
}

impl fmt::Display for DataRootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataRootError::InSyncFolder(p) => {
                write!(f, "the data folder is inside a {p:?} sync folder")
            }
            DataRootError::NoDefault => f.write_str("no default data folder for this user"),
            DataRootError::Io(e) => write!(f, "can't prepare the data folder: {e}"),
        }
    }
}

impl std::error::Error for DataRootError {}

/// A checked data root, with its `jobs/` directory created.
#[derive(Debug, Clone)]
pub struct DataRoot {
    root: PathBuf,
    jobs: PathBuf,
}

impl DataRoot {
    /// Opens the OS default data root.
    pub fn open_default(sync: &SyncRoots) -> Result<Self, DataRootError> {
        Self::open(&default_data_root().ok_or(DataRootError::NoDefault)?, sync)
    }

    /// Opens `root`, refusing it if it's in a sync folder, and creates
    /// `root/jobs` private to the user.
    pub fn open(root: &Path, sync: &SyncRoots) -> Result<Self, DataRootError> {
        if let Some(provider) = sync.provider_for(root) {
            return Err(DataRootError::InSyncFolder(provider));
        }
        let jobs = root.join("jobs");
        ensure_private_dir(root).map_err(DataRootError::Io)?;
        ensure_private_dir(&jobs).map_err(DataRootError::Io)?;
        // Check again after creation: a symlinked root could resolve into a
        // sync folder only once it exists.
        if let Some(provider) = sync.provider_for(root) {
            return Err(DataRootError::InSyncFolder(provider));
        }
        Ok(DataRoot {
            root: normalize(root),
            jobs: normalize(&jobs),
        })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn jobs_dir(&self) -> &Path {
        &self.jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncOs;

    #[test]
    fn refuses_sync_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let sync = SyncRoots::from_roots(
            vec![(tmp.path().join("OneDrive"), SyncProvider::OneDrive)],
            SyncOs::current(),
        );
        let err = DataRoot::open(&tmp.path().join("OneDrive").join("App"), &sync).unwrap_err();
        assert!(matches!(
            err,
            DataRootError::InSyncFolder(SyncProvider::OneDrive)
        ));
        assert!(!tmp.path().join("OneDrive").exists(), "nothing created");
    }

    #[test]
    fn creates_jobs_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::open(&tmp.path().join("App"), &SyncRoots::none()).unwrap();
        assert!(root.jobs_dir().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.jobs_dir())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn default_ends_with_identifier() {
        if let Some(p) = default_data_root() {
            assert!(p.ends_with(APP_DIR_NAME));
        }
    }
}
