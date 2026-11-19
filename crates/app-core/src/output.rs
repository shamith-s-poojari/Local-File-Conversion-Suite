//! Reading a finished job's outputs: clipboard bytes, names, counts.

use std::io::Read;

use converter_jobstore::JobId;
use converter_scheduler::open_regular;

use crate::{
    parse_format, App, CommandError, CommandResult, Reason, CLIPBOARD_SIDE, COPYABLE_PICTURES,
    MAX_CLIPBOARD_PNG,
};

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
