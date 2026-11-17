//! Reading a finished job's outputs: clipboard bytes, names, counts.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use converter_jobstore::JobId;

use crate::{
    parse_format, App, CommandError, CommandResult, Reason, CLIPBOARD_SIDE, COPYABLE_PICTURES,
    MAX_CLIPBOARD_PNG,
};

/// Open flags that keep the open itself from following a link or waiting on
/// a pipe. Spelled out per platform because the core takes no `libc`
/// dependency; a platform not listed gets 0 and relies on the identity
/// check in [`open_regular`] alone.
#[cfg(unix)]
mod flags {
    const LINUX_LIKE: bool = cfg!(any(target_os = "linux", target_os = "android"));
    const BSD_LIKE: bool = cfg!(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ));
    /// x86, riscv, s390x and loongarch share the generic Linux values.
    const GENERIC_ARCH: bool = cfg!(any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "riscv64",
        target_arch = "s390x",
        target_arch = "loongarch64"
    ));
    /// arm, aarch64 and powerpc put O_NOFOLLOW at 0o100000.
    const ARM_LIKE_ARCH: bool = cfg!(any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    ));

    pub const NOFOLLOW: i32 = if BSD_LIKE {
        0x0100
    } else if LINUX_LIKE && GENERIC_ARCH {
        0o400000
    } else if LINUX_LIKE && ARM_LIKE_ARCH {
        0o100000
    } else {
        0
    };
    pub const NONBLOCK: i32 = if BSD_LIKE {
        0x0004
    } else if LINUX_LIKE && (GENERIC_ARCH || ARM_LIKE_ARCH) {
        0o4000
    } else {
        0
    };
}

fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not a plain file")
}

/// Opens a file a worker wrote, refusing anything but a regular file.
///
/// The worker's sandbox can swap a file for a link between a check and an
/// open, so there is no separate check: the open itself doesn't follow a
/// symlink (`O_NOFOLLOW`; on Windows `FILE_FLAG_OPEN_REPARSE_POINT`, which
/// opens a reparse point itself rather than its target), and everything the
/// caller reads comes through the returned handle, whose own metadata is
/// checked. On Unix the handle must also be the file the path names now
/// (same device and inode), which holds even where `O_NOFOLLOW` is 0 here.
pub fn open_regular(path: &Path) -> io::Result<File> {
    // A pipe or device would block or misbehave on open: look first. A link
    // swapped in after this look is caught by the flags and the checks below.
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(refused());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(flags::NOFOLLOW | flags::NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(refused());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(refused());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let now = std::fs::symlink_metadata(path)?;
        if !now.file_type().is_file() || now.dev() != meta.dev() || now.ino() != meta.ino() {
            return Err(refused());
        }
    }
    Ok(file)
}

impl App {
    /// The extension of one output (`png`, `csv`, ...), if it exists.
    pub fn output_extension(&self, job: &JobId, index: usize) -> CommandResult<Option<String>> {
        Ok(self
            .job_snapshot(job)?
            .outputs
            .get(index)
            .map(|o| o.extension.to_ascii_lowercase()))
    }

    /// The bytes of one output, for the clipboard, with its extension. At
    /// most `max_bytes` are read. The path stays inside the core.
    pub fn read_output(
        &self,
        job: &JobId,
        index: usize,
        max_bytes: u64,
    ) -> CommandResult<(String, Vec<u8>)> {
        let snap = self.job_snapshot(job)?;
        if !matches!(
            snap.state,
            converter_jobstore::JobState::Succeeded | converter_jobstore::JobState::Partial
        ) {
            return Err(CommandError::with("E_WRONG_STATE", Reason::JobNotFinished));
        }
        let out = snap
            .outputs
            .get(index)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownOutput))?;
        if out.size_bytes > max_bytes {
            return Err(CommandError::with(
                "E_LIMIT_EXCEEDED",
                Reason::TooLargeToCopy,
            ));
        }
        let path = self
            .store
            .job_dirs(job)
            .map_err(CommandError::from_job)?
            .out
            .join(&out.path);
        let file = open_regular(&path)
            .map_err(|_| CommandError::with("E_INTERNAL", Reason::UnknownOutput))?;
        // `max_bytes + 1` would overflow for a caller passing no limit.
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| CommandError::with("E_INTERNAL", Reason::UnreadableOutput))?;
        if bytes.len() as u64 > max_bytes {
            return Err(CommandError::with(
                "E_LIMIT_EXCEEDED",
                Reason::TooLargeToCopy,
            ));
        }
        Ok((out.extension.clone(), bytes))
    }

    /// A picture output as PNG bytes for the clipboard. The output is
    /// decoded by its own sandboxed worker (the preview path), which writes
    /// a PNG at most [`CLIPBOARD_SIDE`] pixels on its longest side; the
    /// core only reads that PNG back.
    pub fn clipboard_png(&self, job: &JobId, index: usize) -> CommandResult<Vec<u8>> {
        self.require_sandbox()?;
        let snap = self.job_snapshot(job)?;
        if !matches!(
            snap.state,
            converter_jobstore::JobState::Succeeded | converter_jobstore::JobState::Partial
        ) {
            return Err(CommandError::with("E_WRONG_STATE", Reason::JobNotFinished));
        }
        let format = snap
            .outputs
            .get(index)
            .and_then(|o| parse_format(&o.extension))
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownOutput))?;
        if !COPYABLE_PICTURES.contains(&format) {
            return Err(CommandError::with("E_INTERNAL", Reason::NotCopyable));
        }
        let path = self.with_output_worker(job, index, |w, input, tag| {
            w.render_file(input, tag, CLIPBOARD_SIDE / 8)
        })?;
        let mut bytes = Vec::new();
        let read = open_regular(&path).and_then(|f| {
            f.take(MAX_CLIPBOARD_PNG.saturating_add(1))
                .read_to_end(&mut bytes)
        });
        let _ = std::fs::remove_file(&path);
        read.map_err(|_| CommandError::with("E_INTERNAL", Reason::UnreadableOutput))?;
        if bytes.len() as u64 > MAX_CLIPBOARD_PNG {
            return Err(CommandError::with(
                "E_LIMIT_EXCEEDED",
                Reason::TooLargeToCopy,
            ));
        }
        Ok(bytes)
    }

    /// How many outputs a job has (to pick a file or folder dialog).
    pub fn output_count(&self, job: &JobId) -> CommandResult<usize> {
        Ok(self.job_snapshot(job)?.outputs.len())
    }

    /// The suggested name of a single-output job, already sanitized.
    pub fn suggested_name(&self, job: &JobId) -> CommandResult<String> {
        let snap = self.job_snapshot(job)?;
        let out = snap
            .outputs
            .first()
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NoOutputs))?;
        Ok(converter_jobstore::sanitize_file_name(
            &out.suggested_name,
            &out.extension,
            "output",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_a_regular_file() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.bin");
        std::fs::write(&p, b"hello").unwrap();
        let mut s = String::new();
        open_regular(&p).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn refuses_directories_and_missing_files() {
        let t = tempfile::tempdir().unwrap();
        assert!(open_regular(t.path()).is_err());
        assert!(open_regular(&t.path().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_to_a_file_and_a_symlink_in_the_way() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("secret");
        std::fs::write(&target, b"x").unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_regular(&link).is_err());
        let dangling = t.path().join("dangling");
        std::os::unix::fs::symlink(t.path().join("nowhere"), &dangling).unwrap();
        assert!(open_regular(&dangling).is_err());
    }

    /// The open itself refuses a link (not just the look before it): with
    /// the flag alone, the kernel says ELOOP. Only where the flag is known.
    #[cfg(all(
        unix,
        any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))
    ))]
    #[test]
    fn the_open_flag_refuses_a_link_by_itself() {
        use std::os::unix::fs::OpenOptionsExt;
        assert_ne!(flags::NOFOLLOW, 0);
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("t");
        std::fs::write(&target, b"x").unwrap();
        let link = t.path().join("l");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags::NOFOLLOW)
            .open(&link)
            .unwrap_err();
        assert_eq!(
            err.raw_os_error(),
            cfg!(target_os = "linux").then_some(40_i32).or(Some(62))
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_pipe_without_blocking() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&p).status();
        if made.is_ok_and(|s| s.success()) {
            assert!(open_regular(&p).is_err());
        }
    }
}
