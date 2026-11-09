//! Small filesystem helpers: private directories, deletion with retries, and
//! path normalization for comparisons.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::Duration;

/// How deletions are retried when they're blocked (spec section 10.5:
/// "retry 5 times with backoff, then leave for next start"). A blocked delete
/// is common on Windows while an antivirus scanner or indexer holds a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub retries: u32,
    /// Delay before the first retry; doubles after each one.
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            retries: 5,
            base_delay: Duration::from_millis(50),
        }
    }
}

/// Creates a directory readable only by the current user.
///
/// On Unix the mode is `0700`. On Windows the directory inherits the ACL of
/// the per-user data folder (`%LOCALAPPDATA%`), which grants access to the
/// user, SYSTEM and administrators only; the sandbox (T09) adds the job's own
/// AppContainer grant.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

/// Like [`create_private_dir`], but succeeds if the directory already exists
/// and creates missing parents. Existing directories on Unix are tightened to
/// `0700`.
pub(crate) fn ensure_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Deletes a file or directory tree, retrying with backoff while it's
/// blocked. Missing paths count as deleted. Symlinks are removed, never
/// followed.
pub(crate) fn remove_with_retry(path: &Path, policy: RetryPolicy) -> io::Result<()> {
    let mut delay = policy.base_delay;
    let mut attempt = 0;
    loop {
        match remove_once(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt >= policy.retries => return Err(e),
            Err(_) => {
                attempt += 1;
                thread::sleep(delay);
                delay = delay.saturating_mul(2);
            }
        }
    }
}

fn remove_once(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        // `remove_dir_all` doesn't follow symlinks inside the tree.
        std::fs::remove_dir_all(path)
    } else {
        remove_file_or_link(path)
    }
}

fn remove_file_or_link(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        // A directory symlink or junction on Windows is removed with remove_dir.
        Err(e) if cfg!(windows) => std::fs::remove_dir(path).map_err(|_| e),
        Err(e) => Err(e),
    }
}

/// Makes a path comparable with another: absolute, symlinks resolved as far
/// as the path exists, `\\?\` prefixes dropped on Windows. Works for paths
/// that don't exist yet, such as a save destination.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let lexical = lexical_clean(&absolute);

    // Canonicalize the deepest existing ancestor and re-attach the rest.
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            let mut out = strip_verbatim(canonical);
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return lexical,
        }
    }
}

fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(windows)]
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

#[cfg(not(windows))]
fn strip_verbatim(p: PathBuf) -> PathBuf {
    p
}

/// True if `path` is `root` or inside it. Both should already be normalized.
pub(crate) fn is_within(path: &Path, root: &Path, case_insensitive: bool) -> bool {
    let fold = |c: Component<'_>| {
        let s = c.as_os_str().to_string_lossy().into_owned();
        if case_insensitive {
            s.to_lowercase()
        } else {
            s
        }
    };
    let mut p = path.components();
    for r in root.components() {
        match p.next() {
            Some(pc) if fold(pc) == fold(r) => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_compares_whole_components() {
        let root = Path::new("/home/u/Dropbox");
        assert!(is_within(Path::new("/home/u/Dropbox"), root, false));
        assert!(is_within(Path::new("/home/u/Dropbox/a/b"), root, false));
        assert!(!is_within(Path::new("/home/u/Dropbox2/a"), root, false));
        assert!(!is_within(Path::new("/home/u/dropbox/a"), root, false));
        assert!(is_within(Path::new("/home/u/dropbox/a"), root, true));
    }

    #[test]
    fn normalize_handles_missing_tails_and_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        let base = normalize(dir.path());
        let p = dir.path().join("a").join("..").join("b").join("c.pdf");
        assert_eq!(normalize(&p), base.join("b").join("c.pdf"));
    }

    #[test]
    fn remove_missing_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        remove_with_retry(&dir.path().join("nope"), RetryPolicy::default()).unwrap();
    }
}
