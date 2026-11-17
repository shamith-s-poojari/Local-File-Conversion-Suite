//! File workers: the long-lived sandboxed workers that inspect and render
//! one file (or one job output) for previews.
//!
//! A worker is never used under a lock. A call takes it out of its entry
//! (marking the entry busy), runs the request with no lock held, and puts it
//! back; another call for the same file or job waits its turn on a condvar,
//! and calls for other files carry on.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use converter_jobstore::{InputSource, JobFailure, JobId};
use converter_protocol::Secrets;
use converter_sniffer::Format;

use crate::files::{worker_is_broken, FileWorker, InspectInfo, OutputSlot, PageText, Preview};
use crate::{
    lock, parse_format, App, CommandError, CommandResult, Reason, MAX_LIVE_FILE_WORKERS,
    PREVIEW_DPI,
};

/// How long a discard or save waits for a preview that is still rendering.
const OUTPUT_STOP_WAIT: Duration = Duration::from_secs(10);

/// What a call needs after taking its file's worker out of the selection.
struct Claim {
    worker: Option<FileWorker>,
    adapter: &'static str,
    path: PathBuf,
    format: Format,
    size_bytes: u64,
    secrets: Option<Secrets>,
}

impl App {
    /// Shuts a file worker down and discards its hidden job.
    pub(crate) fn close_worker(&self, worker: FileWorker) {
        for token in worker.tokens.values() {
            (self.revoke_preview)(token);
        }
        let job = worker.job.clone();
        worker.shutdown();
        self.discard_hidden(&job);
    }

    /// Fails and deletes a file-worker job, and stops hiding it.
    fn discard_hidden(&self, job: &JobId) {
        let _ = self.store.fail(
            job,
            JobFailure {
                code: "E_CANCELLED".into(),
                retryable: false,
            },
        );
        let _ = self.store.discard(job);
        lock(&self.hidden).remove(job);
    }

    /// Page count, encryption and warnings, from the file's sandboxed worker.
    pub fn inspect_file(&self, file_id: &str) -> CommandResult<InspectInfo> {
        self.refuse_office_password(file_id)?;
        self.require_sandbox()?;
        if self.file_adapter(file_id)?.is_none() {
            return Ok(InspectInfo::unknown());
        }
        self.with_file_worker(file_id, |w, secrets| w.inspect(secrets))
    }

    /// The available adapter that inspects and previews this file, if any.
    pub(crate) fn file_adapter(&self, file_id: &str) -> CommandResult<Option<&'static str>> {
        let format = lock(&self.files)
            .get(file_id)
            .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?
            .descriptor
            .format;
        Ok(self.usable_preview_adapter(format))
    }

    pub(crate) fn usable_preview_adapter(&self, format: Format) -> Option<&'static str> {
        preview_adapter(format).filter(|id| {
            self.registry
                .get(id)
                .is_some_and(|e| self.availability_of(e).is_ok())
        })
    }

    /// Renders a page and returns a `preview:` URL path (`<token>/<page>.png`).
    pub fn request_preview(&self, reference: &str, page: u32, dpi: u32) -> CommandResult<Preview> {
        self.require_sandbox()?;
        if !PREVIEW_DPI.contains(&dpi) {
            return Err(CommandError::with("E_INTERNAL", Reason::UnsupportedDpi));
        }
        let register = self.preview_dirs.clone();
        if let Some((job, output)) = parse_output_ref(reference) {
            return self.with_output_worker(&job, output, |w, input, tag| {
                w.render(input, tag, page, dpi, &*register)
            });
        }
        if self.file_adapter(reference)?.is_none() {
            return Err(CommandError::with("E_INTERNAL", Reason::NotPreviewable));
        }
        self.with_file_worker(reference, |w, secrets| {
            w.render_input(secrets, page, dpi, &*register)
        })
    }

    /// Word boxes for the redaction canvas, in displayed orientation.
    pub fn get_page_text(&self, reference: &str, page: u32) -> CommandResult<PageText> {
        self.require_sandbox()?;
        if let Some((job, output)) = parse_output_ref(reference) {
            return self
                .with_output_worker(&job, output, |w, input, _| w.page_text(input, None, page));
        }
        self.with_file_worker(reference, |w, secrets| {
            let input = w.input.clone();
            w.page_text(&input, secrets, page)
        })
    }

    /// Runs `f` on the file's worker, starting one if needed. No lock is held
    /// while `f` runs, while the worker starts or while the input is staged.
    pub(crate) fn with_file_worker<T>(
        &self,
        file_id: &str,
        f: impl FnOnce(&mut FileWorker, Option<&Secrets>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let (claim, evicted) = self.claim_file(file_id)?;
        for w in evicted {
            self.close_worker(w);
        }
        let Claim {
            worker,
            adapter,
            path,
            format,
            size_bytes,
            secrets,
        } = claim;
        let mut worker = match worker {
            Some(w) => w,
            None => match self.start_file_worker(adapter, &path, format, size_bytes) {
                Ok(w) => w,
                Err(e) => {
                    self.release_file(file_id, None);
                    return Err(e);
                }
            },
        };
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut worker, secrets.as_ref())));
        match result {
            Err(panic) => {
                self.close_worker(worker);
                self.release_file(file_id, None);
                resume_unwind(panic)
            }
            Ok(result) => {
                if matches!(&result, Err(e) if worker_is_broken(&e.code)) {
                    // The worker is gone; a fresh one starts next time.
                    self.close_worker(worker);
                    self.release_file(file_id, None);
                } else {
                    self.release_file(file_id, Some(worker));
                }
                result
            }
        }
    }

    /// Waits until nobody else is using the file's worker, then takes it (or
    /// notes that none runs) and marks the entry busy. Also picks the idle
    /// workers to close so a new one stays under [`MAX_LIVE_FILE_WORKERS`].
    fn claim_file(&self, file_id: &str) -> CommandResult<(Claim, Vec<FileWorker>)> {
        let mut files = lock(&self.files);
        loop {
            match files.get(file_id) {
                None => return Err(CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile)),
                Some(e) if e.busy => {
                    files = self
                        .files_idle
                        .wait(files)
                        .unwrap_or_else(|p| p.into_inner());
                }
                Some(_) => break,
            }
        }
        let mut evicted = Vec::new();
        let needs_worker = files.get(file_id).is_some_and(|e| e.worker.is_none());
        if needs_worker {
            // Close the least recently used idle workers to stay under the cap.
            let others = files
                .iter()
                .filter(|(id, e)| id.as_str() != file_id && (e.worker.is_some() || e.busy))
                .count();
            let mut idle: Vec<(Instant, String)> = files
                .iter()
                .filter(|(id, e)| id.as_str() != file_id && e.worker.is_some() && !e.busy)
                .map(|(id, e)| (e.last_used, id.clone()))
                .collect();
            idle.sort();
            let excess = others
                .saturating_add(1)
                .saturating_sub(MAX_LIVE_FILE_WORKERS);
            for (_, id) in idle.into_iter().take(excess) {
                if let Some(w) = files.get_mut(&id).and_then(|e| e.worker.take()) {
                    evicted.push(w);
                }
            }
        }
        let entry = files.get_mut(file_id).expect("checked above");
        entry.last_used = Instant::now();
        if !entry.descriptor.actionable {
            return Err(CommandError::with("E_INTERNAL", Reason::NotActionable));
        }
        let format = entry.descriptor.format;
        let adapter = self
            .usable_preview_adapter(format)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NotPreviewable))?;
        entry.busy = true;
        let claim = Claim {
            worker: entry.worker.take(),
            adapter,
            path: entry.path.clone(),
            format,
            size_bytes: entry.descriptor.size_bytes,
            secrets: entry.secrets(),
        };
        Ok((claim, evicted))
    }

    /// Puts a worker back (or none) and wakes the calls waiting for the file.
    /// If the file left the selection meanwhile, the worker is closed.
    fn release_file(&self, file_id: &str, worker: Option<FileWorker>) {
        let orphan = match lock(&self.files).get_mut(file_id) {
            Some(entry) => {
                entry.busy = false;
                entry.worker = worker;
                None
            }
            None => worker,
        };
        self.files_idle.notify_all();
        if let Some(w) = orphan {
            self.close_worker(w);
        }
    }

    /// Stages the file in a hidden job and starts its worker. If anything
    /// fails the job is failed and discarded, never left hidden.
    fn start_file_worker(
        &self,
        adapter: &str,
        path: &std::path::Path,
        format: Format,
        size_bytes: u64,
    ) -> CommandResult<FileWorker> {
        let (job, _) = self.store.create_job().map_err(CommandError::from_job)?;
        lock(&self.hidden).insert(job.clone());
        let started = (|| {
            self.store
                .stage_inputs(
                    &job,
                    &[InputSource {
                        path: path.to_path_buf(),
                        extension: format.as_str().into(),
                    }],
                    0.5,
                )
                .map_err(CommandError::from_job)?;
            let dir = self
                .store
                .job_dirs(&job)
                .map_err(CommandError::from_job)?
                .root;
            FileWorker::start(
                &self.launcher,
                self.codec.clone(),
                adapter,
                job.clone(),
                dir,
                format!("in/0.{}", format.as_str()),
                size_bytes,
            )
        })();
        if started.is_err() {
            self.discard_hidden(&job);
        }
        started
    }

    /// Runs `f(worker, input path, preview tag)` on the worker of a finished
    /// job's output. The worker is out of the map while `f` runs, and its
    /// watchdog is sized from this output. After `E_PROTOCOL`, `E_TIMEOUT` or
    /// `E_INSTALL_CORRUPT` it is dropped, and the next call starts a new one.
    pub(crate) fn with_output_worker<T>(
        &self,
        job: &JobId,
        output: usize,
        f: impl FnOnce(&mut FileWorker, &str, &str) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let snapshot = self.job_snapshot(job)?;
        let out = snapshot
            .outputs
            .get(output)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownOutput))?;
        let adapter = parse_format(&out.extension)
            .and_then(|f| self.usable_preview_adapter(f))
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NotPreviewable))?;
        let input = format!("out/{}", out.path);
        // The output's index, not its name: two names can sanitize to one.
        let tag = format!("o{output}");
        let size_bytes = out.size_bytes;

        let (worker, tokens) = self.claim_output(job);
        let mut worker = match worker {
            Some(w) => w,
            None => {
                let started = self
                    .store
                    .job_dirs(job)
                    .map_err(CommandError::from_job)
                    .and_then(|dirs| {
                        FileWorker::start(
                            &self.launcher,
                            self.codec.clone(),
                            adapter,
                            job.clone(),
                            dirs.root,
                            input.clone(),
                            size_bytes,
                        )
                    });
                match started {
                    Ok(w) => w,
                    Err(e) => {
                        self.release_output(job, None, tokens);
                        return Err(e);
                    }
                }
            }
        };
        worker.size_bytes = size_bytes;
        worker.tokens = tokens;
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut worker, &input, &tag)));
        let tokens = std::mem::take(&mut worker.tokens);
        match result {
            Err(panic) => {
                worker.shutdown();
                self.release_output(job, None, tokens);
                resume_unwind(panic)
            }
            Ok(result) => {
                if matches!(&result, Err(e) if worker_is_broken(&e.code)) {
                    worker.shutdown();
                    self.release_output(job, None, tokens);
                } else {
                    self.release_output(job, Some(worker), tokens);
                }
                result
            }
        }
    }

    /// Waits for the job's preview slot to be free, then takes its worker (if
    /// one runs) and tokens, marking the slot busy.
    fn claim_output(
        &self,
        job: &JobId,
    ) -> (
        Option<FileWorker>,
        std::collections::BTreeMap<(String, u32), String>,
    ) {
        let mut workers = lock(&self.output_workers);
        loop {
            match workers.get_mut(job) {
                Some(slot) if slot.busy => {
                    workers = self
                        .output_idle
                        .wait(workers)
                        .unwrap_or_else(|p| p.into_inner());
                }
                Some(slot) => {
                    slot.busy = true;
                    return (slot.worker.take(), std::mem::take(&mut slot.tokens));
                }
                None => {
                    workers.insert(
                        job.clone(),
                        OutputSlot {
                            busy: true,
                            ..OutputSlot::default()
                        },
                    );
                    return (None, Default::default());
                }
            }
        }
    }

    /// Gives the slot back. A slot with nothing in it is dropped; if the job
    /// was closed meanwhile (no slot), the worker and tokens are cleaned up.
    fn release_output(
        &self,
        job: &JobId,
        worker: Option<FileWorker>,
        tokens: std::collections::BTreeMap<(String, u32), String>,
    ) {
        let orphan = {
            let mut workers = lock(&self.output_workers);
            match workers.get_mut(job) {
                Some(slot) if worker.is_some() || !tokens.is_empty() => {
                    slot.busy = false;
                    slot.worker = worker;
                    slot.tokens = tokens;
                    None
                }
                Some(_) => {
                    workers.remove(job);
                    None
                }
                None => Some((worker, tokens)),
            }
        };
        self.output_idle.notify_all();
        if let Some((worker, tokens)) = orphan {
            for token in tokens.values() {
                (self.revoke_preview)(token);
            }
            if let Some(w) = worker {
                w.shutdown();
            }
        }
    }

    /// Stops the job's preview worker process but keeps its preview tokens:
    /// the folders still exist, so previews already on screen keep loading,
    /// and a later preview starts a new worker. Called before a discard or a
    /// save, which must not run with a worker in the job directory (Windows
    /// can't delete it); [`App::forget_output_previews`] follows once that
    /// succeeded. Waits briefly for a render in progress.
    pub(crate) fn stop_output_worker(&self, job: &JobId) {
        let deadline = Instant::now() + OUTPUT_STOP_WAIT;
        let worker = {
            let mut workers = lock(&self.output_workers);
            loop {
                match workers.get_mut(job) {
                    Some(slot) if slot.busy => {
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break None;
                        }
                        workers = self
                            .output_idle
                            .wait_timeout(workers, left)
                            .unwrap_or_else(|p| p.into_inner())
                            .0;
                    }
                    Some(slot) => break slot.worker.take(),
                    None => break None,
                }
            }
        };
        if let Some(w) = worker {
            w.shutdown();
        }
    }

    /// Revokes the job's preview tokens and drops its slot (the job is gone).
    pub(crate) fn forget_output_previews(&self, job: &JobId) {
        let slot = lock(&self.output_workers).remove(job);
        self.output_idle.notify_all();
        if let Some(slot) = slot {
            for token in slot.tokens.values() {
                (self.revoke_preview)(token);
            }
            if let Some(w) = slot.worker {
                w.shutdown();
            }
        }
    }

    /// Stops the worker and forgets the previews (app exit).
    pub(crate) fn close_output_worker(&self, job: &JobId) {
        self.stop_output_worker(job);
        self.forget_output_previews(job);
    }
}

/// `<jobId>:<n>` refers to output `n` of a finished job.
pub(crate) fn parse_output_ref(reference: &str) -> Option<(JobId, usize)> {
    let (job, n) = reference.split_once(':')?;
    Some((JobId::parse(job)?, n.parse().ok()?))
}

/// The adapter whose worker inspects and previews files of this format.
pub(crate) fn preview_adapter(format: Format) -> Option<&'static str> {
    use Format::*;
    match format {
        Pdf => Some("pdf-redact"),
        Png | Jpg | Webp | Svg | Heic | Gif | Bmp | Tiff | Avif | Ico => Some("image-convert"),
        Mp3 | Wav | Flac | Ogg | Opus | M4a | Aac | Aiff => Some("audio-convert"),
        Mp4 | Mov | Mkv | Webm | Avi => Some("video-convert"),
        // What an Office file or a web page reveals or could do (ADR 0005).
        Docx | Xlsx | Pptx | Html => Some("file-disarm"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_refs_parse() {
        let job = JobId::new_random();
        assert_eq!(
            parse_output_ref(&format!("{job}:3")),
            Some((job.clone(), 3))
        );
        assert_eq!(parse_output_ref(&format!("{job}:x")), None);
        assert_eq!(parse_output_ref("../x:1"), None);
        assert_eq!(parse_output_ref("no-colon"), None);
    }

    #[test]
    fn every_previewable_format_has_a_known_adapter() {
        assert_eq!(preview_adapter(Format::Pdf), Some("pdf-redact"));
        assert_eq!(preview_adapter(Format::Docx), Some("file-disarm"));
        assert_eq!(preview_adapter(Format::Csv), None);
    }
}
