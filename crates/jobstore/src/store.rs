//! The authoritative job store (spec sections 10.1, 10.4 and 10.5).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::data_root::DataRoot;
use crate::error::JobError;
use crate::fsutil::{create_private_dir, normalize, remove_with_retry, RetryPolicy};
use crate::id::JobId;
use crate::lease::Lease;
use crate::sync::SyncRoots;

/// Job states (spec section 10.1):
///
/// `created → staging → validating → queued → running → verifying →
/// succeeded | partial | failed | cancelled`
///
/// Any non-terminal state may also go to `failed` or `cancelled`, and
/// `running` may go back to `queued` for the one memory retry
/// (section 10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Created,
    Staging,
    Validating,
    Queued,
    Running,
    Verifying,
    Succeeded,
    Partial,
    Failed,
    Cancelled,
}

impl JobState {
    pub const ALL: [JobState; 10] = [
        JobState::Created,
        JobState::Staging,
        JobState::Validating,
        JobState::Queued,
        JobState::Running,
        JobState::Verifying,
        JobState::Succeeded,
        JobState::Partial,
        JobState::Failed,
        JobState::Cancelled,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::Partial | JobState::Failed | JobState::Cancelled
        )
    }

    /// True if the state machine allows `self → to`.
    pub fn can_go_to(self, to: JobState) -> bool {
        use JobState::*;
        if self.is_terminal() {
            return false;
        }
        if matches!(to, Failed | Cancelled) {
            return true;
        }
        matches!(
            (self, to),
            (Created, Staging)
                | (Staging, Validating)
                | (Validating, Queued)
                | (Queued, Running)
                | (Running, Queued)
                | (Running, Verifying)
                | (Verifying, Succeeded)
                | (Verifying, Partial)
        )
    }
}

/// A warning attached to a job. `detail` is worker text: the UI renders it as
/// plain text only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Progress within the current stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// Why a job failed: a catalog code (spec section 13.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFailure {
    pub code: String,
    pub retryable: bool,
}

/// One change to a job (spec section 11.2). `seq` increases by one for every
/// event in this app run, across all jobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub seq: u64,
    pub job_id: JobId,
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// Warnings added by this change only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JobFailure>,
}

/// A file a job produced, relative to its `out/` directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    /// Path relative to `out/`, with `/` separators, such as `0.pdf`.
    pub path: String,
    /// The worker's suggestion; sanitized when saving.
    pub suggested_name: String,
    /// Extension of the declared output format, such as `pdf`.
    pub extension: String,
    pub size_bytes: u64,
}

/// A job as the UI sees it after a reload (`get_snapshot`). Holds no paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub job_id: JobId,
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JobFailure>,
    pub outputs: Vec<OutputFile>,
}

/// Every job, and the latest `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub seq: u64,
    pub jobs: Vec<JobSnapshot>,
}

/// A file to stage into a job.
#[derive(Debug, Clone)]
pub struct InputSource {
    /// The user's file, from the native picker or drop event.
    pub path: PathBuf,
    /// Extension for the staged copy, from the sniffed format, such as `pdf`.
    pub extension: String,
}

/// A job's directories, for the supervisor and sandbox. Absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDirs {
    pub root: PathBuf,
    pub input_dir: PathBuf,
    /// Staged inputs in order: `in/0.pdf`, `in/1.pdf`, …
    pub inputs: Vec<PathBuf>,
    pub work: PathBuf,
    pub out: PathBuf,
}

/// Receives every event as it happens, in `seq` order. Called with the store
/// locked, so it must not call back into the store.
pub type EventSink = Arc<dyn Fn(&JobEvent) + Send + Sync>;

/// Job store settings.
#[derive(Clone)]
pub struct StoreConfig {
    pub retry: RetryPolicy,
    /// A job directory without a readable lease is collected at startup only
    /// once it's this old, in case another instance is still creating it.
    pub orphan_grace: Duration,
    pub on_event: Option<EventSink>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        StoreConfig {
            retry: RetryPolicy::default(),
            orphan_grace: Duration::from_secs(60),
            on_event: None,
        }
    }
}

impl std::fmt::Debug for StoreConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreConfig")
            .field("retry", &self.retry)
            .field("orphan_grace", &self.orphan_grace)
            .field("on_event", &self.on_event.is_some())
            .finish()
    }
}

/// What the startup sweep did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Directories of dead owners that were deleted.
    pub removed: usize,
    /// Directories left alone because their owner is alive or they're new.
    pub kept: usize,
    /// Directories that couldn't be deleted; tried again next start.
    pub failed: usize,
}

/// What shutdown did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    pub removed: usize,
    /// Left for the next start's sweep.
    pub failed: usize,
}

pub(crate) struct StagedInput {
    pub(crate) rel: PathBuf,
    /// The user's file. Kept in memory only, to warn before saving over it.
    pub(crate) original: PathBuf,
}

pub(crate) struct Job {
    pub(crate) id: JobId,
    pub(crate) state: JobState,
    stage: Option<String>,
    progress: Option<Progress>,
    warnings: Vec<Warning>,
    error: Option<JobFailure>,
    pub(crate) dir: PathBuf,
    pub(crate) inputs: Vec<StagedInput>,
    pub(crate) outputs: Vec<OutputFile>,
    /// A save or discard is in progress.
    pub(crate) busy: bool,
}

pub(crate) struct Inner {
    seq: u64,
    pub(crate) jobs: BTreeMap<JobId, Job>,
    /// Directories whose deletion failed; tried again at shutdown.
    pub(crate) leftovers: Vec<PathBuf>,
}

/// The authoritative, in-memory job store. Nothing about jobs persists on
/// disk except their directories, which are deleted at the latest on the
/// next start.
pub struct JobStore {
    pub(crate) data_root: DataRoot,
    pub(crate) sync: SyncRoots,
    lease: Lease,
    pub(crate) config: StoreConfig,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for JobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobStore")
            .field("lease", &self.lease)
            .finish_non_exhaustive()
    }
}

impl JobStore {
    /// Opens the store and collects job directories left by dead processes
    /// (spec section 10.5, "App start").
    pub fn open(
        data_root: DataRoot,
        sync: SyncRoots,
        config: StoreConfig,
    ) -> Result<(JobStore, SweepReport), JobError> {
        let lease = Lease::current().map_err(JobError::io("reading the process start time"))?;
        let report = sweep(data_root.jobs_dir(), &lease, &config)?;
        let store = JobStore {
            data_root,
            sync,
            lease,
            config,
            inner: Mutex::new(Inner {
                seq: 0,
                jobs: BTreeMap::new(),
                leftovers: Vec::new(),
            }),
        };
        Ok((store, report))
    }

    pub fn data_root(&self) -> &DataRoot {
        &self.data_root
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves the data consistent enough:
        // every mutation is a single assignment or push.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Creates a job and its private directory with `in/`, `work/`, `out/`
    /// and a lease.
    pub fn create_job(&self) -> Result<(JobId, JobEvent), JobError> {
        let id = JobId::new_random();
        let dir = self.data_root.jobs_dir().join(id.as_str());
        create_private_dir(&dir).map_err(JobError::io("creating the job directory"))?;
        let setup = || -> std::io::Result<()> {
            self.lease.write_to(&dir)?;
            for sub in ["in", "work", "out"] {
                create_private_dir(&dir.join(sub))?;
            }
            Ok(())
        };
        if let Err(e) = setup() {
            let _ = remove_with_retry(&dir, self.config.retry);
            return Err(JobError::io("preparing the job directory")(e));
        }

        let mut inner = self.lock();
        let job = Job {
            id: id.clone(),
            state: JobState::Created,
            stage: None,
            progress: None,
            warnings: Vec::new(),
            error: None,
            dir,
            inputs: Vec::new(),
            outputs: Vec::new(),
            busy: false,
        };
        let event = self.emit(&mut inner.seq, &job, Vec::new());
        inner.jobs.insert(id.clone(), job);
        Ok((id, event))
    }

    /// Moves a `created` job to `staging`, checks free space, and copies the
    /// inputs to `in/0.<ext>`, `in/1.<ext>`, … using a filesystem clone
    /// where available (spec section 10.4, change C3).
    ///
    /// Free space must be at least `sum(input size) × (1 + disk_factor)`.
    /// The job stays in `staging` on success. On error the caller fails the
    /// job with [`JobError::code`], which also deletes the partial copies.
    pub fn stage_inputs(
        &self,
        id: &JobId,
        sources: &[InputSource],
        disk_factor: f64,
    ) -> Result<Vec<JobEvent>, JobError> {
        if sources.is_empty() {
            return Err(JobError::InvalidInput("no input files"));
        }
        if !disk_factor.is_finite() || disk_factor < 0.0 {
            return Err(JobError::InvalidInput("disk factor must be zero or more"));
        }
        for s in sources {
            let ext_ok = (1..=10).contains(&s.extension.len())
                && s.extension
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
            if !ext_ok {
                return Err(JobError::InvalidInput("bad staging extension"));
            }
        }

        let mut events = Vec::new();
        let dir = {
            let mut inner = self.lock();
            let Inner { seq, jobs, .. } = &mut *inner;
            let job = jobs
                .get_mut(id)
                .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
            check_transition(job, JobState::Staging)?;
            job.state = JobState::Staging;
            job.stage = Some("preparing".to_owned());
            events.push(self.emit(seq, job, Vec::new()));
            job.dir.clone()
        };

        let mut total: u64 = 0;
        for s in sources {
            let meta = std::fs::metadata(&s.path).map_err(JobError::io("reading an input"))?;
            if !meta.is_file() {
                return Err(JobError::InvalidInput("an input is not a regular file"));
            }
            total = total.saturating_add(meta.len());
        }
        let required = (total as f64 * (1.0 + disk_factor)).ceil();
        let required = if required >= u64::MAX as f64 {
            u64::MAX
        } else {
            required as u64
        };
        let available =
            fs4::available_space(&dir).map_err(JobError::io("checking free disk space"))?;
        if required > available {
            return Err(JobError::InsufficientDisk {
                required,
                available,
            });
        }

        let mut staged = Vec::with_capacity(sources.len());
        for (i, s) in sources.iter().enumerate() {
            // Stop early if the job was cancelled or failed meanwhile.
            self.expect_state(id, JobState::Staging)?;
            let rel = PathBuf::from("in").join(format!("{i}.{}", s.extension));
            let dest = dir.join(&rel);
            reflink_copy::reflink_or_copy(&s.path, &dest)
                .map_err(JobError::io("copying an input"))?;
            let mut perms = std::fs::metadata(&dest)
                .map_err(JobError::io("reading a staged input"))?
                .permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&dest, perms)
                .map_err(JobError::io("protecting a staged input"))?;
            staged.push(StagedInput {
                rel,
                original: normalize(&s.path),
            });
        }

        let mut inner = self.lock();
        let Inner { seq, jobs, .. } = &mut *inner;
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        if job.state != JobState::Staging {
            return Err(JobError::WrongState {
                needed: "staging",
                actual: job.state,
            });
        }
        job.inputs = staged;
        job.stage = None;
        events.push(self.emit(seq, job, Vec::new()));
        Ok(events)
    }

    /// Changes a job's state. Use [`JobStore::fail`] for failures, so the
    /// event carries a code. Entering a terminal state runs the section 10.5
    /// cleanup.
    pub fn transition(&self, id: &JobId, to: JobState) -> Result<JobEvent, JobError> {
        self.change_state(id, to, None)
    }

    /// Fails a job with a catalog code, and deletes its `in/`, `work/` and
    /// `out/`.
    pub fn fail(&self, id: &JobId, failure: JobFailure) -> Result<JobEvent, JobError> {
        self.change_state(id, JobState::Failed, Some(failure))
    }

    fn change_state(
        &self,
        id: &JobId,
        to: JobState,
        failure: Option<JobFailure>,
    ) -> Result<JobEvent, JobError> {
        let (event, cleanup, recreate) = {
            let mut inner = self.lock();
            let Inner { seq, jobs, .. } = &mut *inner;
            let job = jobs
                .get_mut(id)
                .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
            check_transition(job, to)?;
            let retrying = job.state == JobState::Running && to == JobState::Queued;
            job.state = to;
            job.stage = None;
            job.progress = None;
            if to == JobState::Queued {
                // Memory retry: the next run starts clean.
                job.outputs.clear();
            }
            if matches!(to, JobState::Failed | JobState::Cancelled) {
                job.outputs.clear();
                job.inputs.clear();
            }
            job.error = failure;
            let event = self.emit(seq, job, Vec::new());
            let cleanup: Vec<PathBuf> = match to {
                JobState::Succeeded | JobState::Partial => vec![job.dir.join("work")],
                JobState::Failed | JobState::Cancelled => ["in", "work", "out"]
                    .iter()
                    .map(|s| job.dir.join(s))
                    .collect(),
                // The retry starts with empty work/ and out/.
                JobState::Queued if retrying => vec![job.dir.join("work"), job.dir.join("out")],
                _ => Vec::new(),
            };
            let recreate = if retrying {
                cleanup.clone()
            } else {
                Vec::new()
            };
            (event, cleanup, recreate)
        };
        for path in cleanup {
            if remove_with_retry(&path, self.config.retry).is_err() {
                self.lock().leftovers.push(path);
            }
        }
        for path in recreate {
            if !path.exists() {
                create_private_dir(&path).map_err(JobError::io("resetting a job directory"))?;
            }
        }
        Ok(event)
    }

    /// Sets the current stage name, clearing progress.
    pub fn set_stage(&self, id: &JobId, name: &str) -> Result<JobEvent, JobError> {
        self.update_active(id, |job| {
            job.stage = Some(name.to_owned());
            job.progress = None;
            Vec::new()
        })
    }

    /// Records progress within the current stage. Throttling to 10 Hz is the
    /// event forwarder's job.
    pub fn set_progress(&self, id: &JobId, progress: Progress) -> Result<JobEvent, JobError> {
        self.update_active(id, |job| {
            job.progress = Some(progress);
            Vec::new()
        })
    }

    /// Adds a warning. The event carries just this warning.
    pub fn add_warning(&self, id: &JobId, warning: Warning) -> Result<JobEvent, JobError> {
        self.update_active(id, |job| {
            job.warnings.push(warning.clone());
            vec![warning]
        })
    }

    fn update_active(
        &self,
        id: &JobId,
        f: impl FnOnce(&mut Job) -> Vec<Warning>,
    ) -> Result<JobEvent, JobError> {
        let mut inner = self.lock();
        let Inner { seq, jobs, .. } = &mut *inner;
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        if job.state.is_terminal() {
            return Err(JobError::WrongState {
                needed: "active",
                actual: job.state,
            });
        }
        let new = f(job);
        Ok(self.emit(seq, job, new))
    }

    /// Records a job's outputs while it's `verifying`. Each must be a
    /// regular file inside `out/`, reached without symlinks or reparse
    /// points, with the claimed size (spec section 7.4).
    ///
    /// On error, the caller fails the job with [`JobError::code`]
    /// (`E_PROTOCOL`).
    pub fn record_outputs(&self, id: &JobId, outputs: Vec<OutputFile>) -> Result<(), JobError> {
        let dir = {
            let inner = self.lock();
            let job = inner
                .jobs
                .get(id)
                .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
            if job.state != JobState::Verifying {
                return Err(JobError::WrongState {
                    needed: "verifying",
                    actual: job.state,
                });
            }
            job.dir.clone()
        };
        let out = dir.join("out");
        for o in &outputs {
            check_output(&out, o)?;
        }
        let mut inner = self.lock();
        let job = inner
            .jobs
            .get_mut(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        if job.state != JobState::Verifying {
            return Err(JobError::WrongState {
                needed: "verifying",
                actual: job.state,
            });
        }
        job.outputs = outputs;
        Ok(())
    }

    /// The job's directories, for the supervisor and sandbox.
    pub fn job_dirs(&self, id: &JobId) -> Result<JobDirs, JobError> {
        let inner = self.lock();
        let job = inner
            .jobs
            .get(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        Ok(JobDirs {
            root: job.dir.clone(),
            input_dir: job.dir.join("in"),
            inputs: job.inputs.iter().map(|i| job.dir.join(&i.rel)).collect(),
            work: job.dir.join("work"),
            out: job.dir.join("out"),
        })
    }

    /// Current state of every job, and the latest `seq` (spec section 11.1,
    /// `get_snapshot`).
    pub fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            seq: inner.seq,
            jobs: inner
                .jobs
                .values()
                .map(|j| JobSnapshot {
                    job_id: j.id.clone(),
                    state: j.state,
                    stage: j.stage.clone(),
                    progress: j.progress,
                    warnings: j.warnings.clone(),
                    error: j.error.clone(),
                    outputs: j.outputs.clone(),
                })
                .collect(),
        }
    }

    /// Deletes a finished job and its directory (spec section 10.5,
    /// "discarded"). Cancel and stop the worker first.
    pub fn discard(&self, id: &JobId) -> Result<(), JobError> {
        let dir = {
            let mut inner = self.lock();
            let job = inner
                .jobs
                .get(id)
                .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
            if !job.state.is_terminal() {
                return Err(JobError::WrongState {
                    needed: "finished",
                    actual: job.state,
                });
            }
            if job.busy {
                return Err(JobError::Busy);
            }
            inner.jobs.remove(id).map(|j| j.dir).expect("checked above")
        };
        self.remove_dir_or_defer(dir);
        Ok(())
    }

    pub(crate) fn remove_dir_or_defer(&self, dir: PathBuf) -> bool {
        match remove_with_retry(&dir, self.config.retry) {
            Ok(()) => true,
            Err(_) => {
                self.lock().leftovers.push(dir);
                false
            }
        }
    }

    /// Deletes every job directory this process owns (spec section 10.5,
    /// "App exit"). Kill all workers first. Anything that can't be deleted
    /// is left for the next start.
    pub fn shutdown(&self) -> ShutdownReport {
        let mut dirs: Vec<PathBuf> = {
            let mut inner = self.lock();
            let mut dirs: Vec<PathBuf> = std::mem::take(&mut inner.jobs)
                .into_values()
                .map(|j| j.dir)
                .collect();
            dirs.append(&mut inner.leftovers);
            dirs
        };
        // Also catch directories this process created but no longer tracks.
        if let Ok(entries) = std::fs::read_dir(self.data_root.jobs_dir()) {
            for e in entries.flatten() {
                let p = e.path();
                if Lease::read_from(&p) == Some(self.lease) && !dirs.contains(&p) {
                    dirs.push(p);
                }
            }
        }
        let mut report = ShutdownReport::default();
        for dir in dirs {
            match remove_with_retry(&dir, self.config.retry) {
                Ok(()) => report.removed += 1,
                Err(_) => report.failed += 1,
            }
        }
        report
    }

    fn expect_state(&self, id: &JobId, state: JobState) -> Result<(), JobError> {
        let inner = self.lock();
        let job = inner
            .jobs
            .get(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        if job.state == state {
            Ok(())
        } else {
            Err(JobError::WrongState {
                needed: "staging",
                actual: job.state,
            })
        }
    }

    fn emit(&self, seq: &mut u64, job: &Job, warnings: Vec<Warning>) -> JobEvent {
        *seq += 1;
        let event = JobEvent {
            seq: *seq,
            job_id: job.id.clone(),
            state: job.state,
            stage: job.stage.clone(),
            progress: job.progress,
            warnings,
            error: job.error.clone(),
        };
        if let Some(sink) = &self.config.on_event {
            sink(&event);
        }
        event
    }
}

fn check_transition(job: &Job, to: JobState) -> Result<(), JobError> {
    if job.busy || !job.state.can_go_to(to) {
        return Err(JobError::InvalidTransition {
            from: job.state,
            to,
        });
    }
    Ok(())
}

fn check_output(out: &Path, o: &OutputFile) -> Result<(), JobError> {
    let rel = Path::new(&o.path);
    if o.path.is_empty()
        || o.path.contains('\\')
        || !rel.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(JobError::InvalidOutput(
            "path is not a plain path inside out/",
        ));
    }
    // Every step must be a real directory or file, not a link.
    let mut current = out.to_path_buf();
    for c in rel.components() {
        current.push(c);
        let meta = std::fs::symlink_metadata(&current)
            .map_err(|_| JobError::InvalidOutput("output file is missing"))?;
        if meta.file_type().is_symlink() || is_reparse_point(&meta) {
            return Err(JobError::InvalidOutput("output path contains a link"));
        }
    }
    let meta = std::fs::symlink_metadata(&current)
        .map_err(|_| JobError::InvalidOutput("output file is missing"))?;
    if !meta.is_file() {
        return Err(JobError::InvalidOutput("output is not a regular file"));
    }
    if meta.len() != o.size_bytes {
        return Err(JobError::InvalidOutput(
            "output size differs from the claim",
        ));
    }
    let real = std::fs::canonicalize(&current)
        .map_err(|_| JobError::InvalidOutput("output file is missing"))?;
    let real_out =
        std::fs::canonicalize(out).map_err(JobError::io("resolving the output folder"))?;
    if !real.starts_with(&real_out) {
        return Err(JobError::InvalidOutput("output resolves outside out/"));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &std::fs::Metadata) -> bool {
    false
}

/// Deletes job directories whose owner is gone (spec section 10.5, "App
/// start").
fn sweep(jobs_dir: &Path, own: &Lease, config: &StoreConfig) -> Result<SweepReport, JobError> {
    let mut report = SweepReport::default();
    let entries = std::fs::read_dir(jobs_dir).map_err(JobError::io("listing job directories"))?;
    let now = SystemTime::now();
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let orphan = if meta.is_dir() {
            match Lease::read_from(&path) {
                Some(lease) if lease == *own => false,
                Some(lease) => !lease.is_alive(),
                None => meta
                    .modified()
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .is_none_or(|age| age >= config.orphan_grace),
            }
        } else {
            // Stray files and links don't belong here.
            true
        };
        if !orphan {
            report.kept += 1;
        } else if remove_with_retry(&path, config.retry).is_ok() {
            report.removed += 1;
        } else {
            report.failed += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_table() {
        use JobState::*;
        let allowed = [
            (Created, Staging),
            (Staging, Validating),
            (Validating, Queued),
            (Queued, Running),
            (Running, Queued),
            (Running, Verifying),
            (Verifying, Succeeded),
            (Verifying, Partial),
        ];
        for from in JobState::ALL {
            for to in JobState::ALL {
                let want = !from.is_terminal()
                    && (matches!(to, Failed | Cancelled) || allowed.contains(&(from, to)));
                assert_eq!(from.can_go_to(to), want, "{from:?} -> {to:?}");
            }
        }
    }
}
