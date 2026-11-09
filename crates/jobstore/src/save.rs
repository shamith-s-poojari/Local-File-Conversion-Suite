//! Saving outputs (spec section 10.6).
//!
//! The native save dialog runs in Rust (the app shell); this module takes the
//! path it returns. The renderer never sees or supplies a path.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use tempfile::NamedTempFile;

use crate::error::JobError;
use crate::fsutil::{is_within, normalize};
use crate::id::JobId;
use crate::name::sanitize_file_name;
use crate::store::{JobState, JobStore, OutputFile};
use crate::sync::{SyncOs, SyncProvider};

/// Where the user chose to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveDestination {
    /// One output, to this exact path from the save dialog. The dialog has
    /// already confirmed replacing an existing file, so it's replaced.
    File(PathBuf),
    /// Any number of outputs into this folder from the folder dialog. Name
    /// collisions get a numeric suffix; nothing is ever replaced.
    Folder(PathBuf),
}

/// Confirmations the user gave after seeing a [`SaveCheck`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// The user confirmed replacing one of the job's own input files.
    pub confirmed_input_overwrite: bool,
}

/// What the UI must show before saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCheck {
    /// The destination is in a sync folder: show "This folder syncs to the
    /// cloud." A notice only; it doesn't block saving.
    pub sync_provider: Option<SyncProvider>,
    /// The destination is one of the job's inputs: ask for explicit
    /// confirmation, then pass [`SaveOptions::confirmed_input_overwrite`].
    pub overwrites_input: bool,
}

/// The result of a save. The job and its directory are gone afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    /// Where each output went, in output order.
    pub saved: Vec<PathBuf>,
    pub sync_provider: Option<SyncProvider>,
    /// The job directory couldn't be deleted yet; it's retried at exit and
    /// at the next start.
    pub cleanup_deferred: bool,
}

/// Most numeric suffixes to try before giving up on a folder save.
const MAX_SUFFIX: u32 = 9999;

impl JobStore {
    /// Checks a destination before saving, for the sync notice and the
    /// input-overwrite confirmation.
    pub fn check_save(&self, id: &JobId, dest: &SaveDestination) -> Result<SaveCheck, JobError> {
        let (_, outputs, originals) = self.saveable(id)?;
        check_shape(&outputs, dest)?;
        Ok(SaveCheck {
            sync_provider: self.sync.provider_for(dest_path(dest)),
            overwrites_input: overwrites_input(dest, &originals),
        })
    }

    /// Saves a finished job's outputs atomically, then deletes the job
    /// (spec section 10.5, "Saved").
    ///
    /// Each output is copied to a temporary file in the destination folder,
    /// flushed to disk, then renamed into place, so a crash never leaves a
    /// half-written file under the final name and a save to another volume
    /// is a copy followed by a same-volume rename. If any output fails, the
    /// files this call already saved are removed and the job stays, so the
    /// user can try again.
    pub fn save_outputs(
        &self,
        id: &JobId,
        dest: &SaveDestination,
        options: SaveOptions,
    ) -> Result<SaveReport, JobError> {
        let (dir, outputs, originals) = self.saveable(id)?;
        check_shape(&outputs, dest)?;
        if overwrites_input(dest, &originals) && !options.confirmed_input_overwrite {
            return Err(JobError::NeedsInputOverwriteConfirmation);
        }

        self.set_busy(id, true)?;
        let out = dir.join("out");
        let result = match dest {
            SaveDestination::File(path) => save_file(&out, &outputs[0], path).map(|p| vec![p]),
            SaveDestination::Folder(folder) => save_into_folder(&out, &outputs, folder),
        };
        let saved = match result {
            Ok(saved) => saved,
            Err(e) => {
                let _ = self.set_busy(id, false);
                return Err(e);
            }
        };

        let dir = {
            let mut inner = self.lock();
            inner.jobs.remove(id).map(|j| j.dir)
        };
        let cleanup_deferred = match dir {
            Some(dir) => !self.remove_dir_or_defer(dir),
            None => false,
        };
        Ok(SaveReport {
            saved,
            sync_provider: self.sync.provider_for(dest_path(dest)),
            cleanup_deferred,
        })
    }

    /// The job's directory, outputs and original input paths, if it can be
    /// saved.
    fn saveable(&self, id: &JobId) -> Result<(PathBuf, Vec<OutputFile>, Vec<PathBuf>), JobError> {
        let inner = self.lock();
        let job = inner
            .jobs
            .get(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        if !matches!(job.state, JobState::Succeeded | JobState::Partial) {
            return Err(JobError::WrongState {
                needed: "succeeded or partial",
                actual: job.state,
            });
        }
        if job.busy {
            return Err(JobError::Busy);
        }
        if job.outputs.is_empty() {
            return Err(JobError::InvalidInput("the job has no outputs to save"));
        }
        Ok((
            job.dir.clone(),
            job.outputs.clone(),
            job.inputs.iter().map(|i| i.original.clone()).collect(),
        ))
    }

    fn set_busy(&self, id: &JobId, busy: bool) -> Result<(), JobError> {
        let mut inner = self.lock();
        let job = inner
            .jobs
            .get_mut(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        if busy && job.busy {
            return Err(JobError::Busy);
        }
        job.busy = busy;
        Ok(())
    }
}

fn dest_path(dest: &SaveDestination) -> &Path {
    match dest {
        SaveDestination::File(p) | SaveDestination::Folder(p) => p,
    }
}

fn check_shape(outputs: &[OutputFile], dest: &SaveDestination) -> Result<(), JobError> {
    if matches!(dest, SaveDestination::File(_)) && outputs.len() != 1 {
        return Err(JobError::InvalidInput(
            "several outputs must be saved to a folder",
        ));
    }
    Ok(())
}

fn overwrites_input(dest: &SaveDestination, originals: &[PathBuf]) -> bool {
    // Saving into a folder never replaces anything.
    let SaveDestination::File(dest) = dest else {
        return false;
    };
    let case_insensitive = SyncOs::current() != SyncOs::Linux;
    let dest_norm = normalize(dest);
    originals.iter().any(|orig| {
        // Same file through a hard link or a different spelling.
        same_file::is_same_file(dest, orig).unwrap_or(false)
            || (is_within(&dest_norm, orig, case_insensitive)
                && dest_norm.components().count() == orig.components().count())
    })
}

fn save_file(out: &Path, output: &OutputFile, dest: &Path) -> Result<PathBuf, JobError> {
    let folder = parent_folder(dest)?;
    let tmp = copy_to_temp(out, output, folder)?;
    tmp.persist(dest)
        .map_err(|e| JobError::io("saving the file")(e.error))?;
    sync_dir(folder);
    Ok(dest.to_path_buf())
}

fn save_into_folder(
    out: &Path,
    outputs: &[OutputFile],
    folder: &Path,
) -> Result<Vec<PathBuf>, JobError> {
    let meta = std::fs::metadata(folder).map_err(JobError::io("opening the save folder"))?;
    if !meta.is_dir() {
        return Err(JobError::InvalidInput(
            "the save destination is not a folder",
        ));
    }
    let mut saved = Vec::with_capacity(outputs.len());
    for (n, output) in outputs.iter().enumerate() {
        match save_one_into_folder(out, output, n, folder) {
            Ok(p) => saved.push(p),
            Err(e) => {
                for p in &saved {
                    let _ = std::fs::remove_file(p);
                }
                return Err(e);
            }
        }
    }
    sync_dir(folder);
    Ok(saved)
}

fn save_one_into_folder(
    out: &Path,
    output: &OutputFile,
    n: usize,
    folder: &Path,
) -> Result<PathBuf, JobError> {
    let name = sanitize_file_name(
        &output.suggested_name,
        &output.extension,
        &format!("output-{}", n + 1),
    );
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let mut tmp = copy_to_temp(out, output, folder)?;
    for k in 0..=MAX_SUFFIX {
        let candidate = if k == 0 {
            folder.join(&name)
        } else {
            folder.join(format!("{stem} ({k}){ext}"))
        };
        match tmp.persist_noclobber(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => tmp = e.file,
            Err(e) => return Err(JobError::io("saving a file")(e.error)),
        }
    }
    Err(JobError::InvalidInput("too many files with the same name"))
}

fn parent_folder(dest: &Path) -> Result<&Path, JobError> {
    let folder = match dest.parent() {
        Some(p) if p.as_os_str().is_empty() => Path::new("."),
        Some(p) => p,
        None => return Err(JobError::InvalidInput("the save path has no folder")),
    };
    if dest.file_name().is_none() {
        return Err(JobError::InvalidInput("the save path has no file name"));
    }
    Ok(folder)
}

/// Copies one output into a new temporary file in `folder`, flushed to disk.
fn copy_to_temp(out: &Path, output: &OutputFile, folder: &Path) -> Result<NamedTempFile, JobError> {
    let src_path = out.join(&output.path);
    // record_outputs checked the path; refuse a link swapped in since then.
    let meta = std::fs::symlink_metadata(&src_path).map_err(JobError::io("reading an output"))?;
    if !meta.is_file() {
        return Err(JobError::InvalidOutput("output is not a regular file"));
    }
    let mut src = File::open(&src_path).map_err(JobError::io("reading an output"))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".lfcs-")
        .suffix(".tmp")
        .tempfile_in(folder)
        .map_err(JobError::io("creating a file in the save folder"))?;
    let copied =
        io::copy(&mut src, tmp.as_file_mut()).map_err(JobError::io("writing the saved file"))?;
    if copied != output.size_bytes {
        return Err(JobError::InvalidOutput("output changed after verification"));
    }
    tmp.as_file_mut()
        .flush()
        .and_then(|()| tmp.as_file().sync_all())
        .map_err(JobError::io("flushing the saved file"))?;
    Ok(tmp)
}

/// Makes a rename durable on Unix. Best effort; Windows has no equivalent.
fn sync_dir(_folder: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = File::open(_folder) {
        let _ = dir.sync_all();
    }
}
