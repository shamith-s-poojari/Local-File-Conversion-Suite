//! The selection: adding files, folders and pasted items; removing them;
//! passwords and format resolution.
//!
//! The selection lock (`App::files`) guards the map only. Reading, sniffing,
//! writing and downloading happen before or after it is held, never during.

use std::path::{Path, PathBuf};

use converter_router::Format as RouterFormat;
use converter_sniffer::{FileDescriptor, Format};
use zeroize::Zeroizing;

use crate::dupes;
use crate::files::{is_office_password, FileEntry, InspectInfo};
use crate::{
    intake, lock, parse_format, App, CommandError, CommandResult, FileResult, Reason, MAX_FILES,
};

impl App {
    // --------------------------------------------------------------- files

    /// Sniffs paths from the native picker or a drop (spec 6). Rejected
    /// files come back with an error; the others get a `fileId`.
    ///
    /// A folder is walked (see [`intake::walk`]) and its files are added with
    /// their `folder`. What doesn't fit under [`MAX_FILES`] is left out and
    /// reported as one extra result: `error: "folderTruncated"` with `count`.
    /// An empty folder comes back as `error: "emptyFolder"`.
    pub fn add_paths(&self, paths: Vec<PathBuf>) -> CommandResult<Vec<FileResult>> {
        let mut items: Vec<Intake> = Vec::new();
        let mut results = Vec::new();
        let mut has_folder = false;
        let mut first_root: Option<String> = None;
        for path in paths {
            if path.is_dir() {
                has_folder = true;
                let walked = intake::walk(&path);
                first_root.get_or_insert_with(|| walked.root.clone());
                if walked.files.is_empty() {
                    results.push(FileResult {
                        file: None,
                        error: Some("emptyFolder".into()),
                        display_name: walked.root.clone(),
                        count: None,
                    });
                }
                for f in &walked.files {
                    items.push(Intake {
                        display_name: file_name_of(&f.path),
                        folder: Some(f.folder(&walked.root)),
                        path: f.path.clone(),
                        temp: false,
                    });
                }
            } else {
                items.push(Intake {
                    display_name: file_name_of(&path),
                    folder: None,
                    path,
                    temp: false,
                });
            }
        }
        let room = MAX_FILES.saturating_sub(lock(&self.files).len());
        if !has_folder && items.len() > room {
            return Err(CommandError::with("E_LIMIT_EXCEEDED", Reason::TooManyFiles));
        }
        let mut left_out = items.len().saturating_sub(room);
        items.truncate(room);
        // Reading the first bytes of every file can be slow (a network
        // share, a cloud placeholder): no lock while it happens.
        let sniffed: Vec<Sniffed> = items.into_iter().map(sniff).collect();
        {
            let mut files = lock(&self.files);
            for s in sniffed {
                match s {
                    Sniffed::Accepted(entry, result) => {
                        // Another call may have filled the selection meanwhile.
                        if files.len() >= MAX_FILES {
                            left_out = left_out.saturating_add(1);
                        } else {
                            files.insert(entry.descriptor.file_id.clone(), *entry);
                            results.push(result);
                        }
                    }
                    Sniffed::Rejected(result) => results.push(result),
                }
            }
        }
        if left_out > 0 {
            results.push(FileResult {
                file: None,
                error: Some("folderTruncated".into()),
                display_name: first_root.unwrap_or_default(),
                count: Some(left_out),
            });
        }
        Ok(results)
    }

    /// Adds something pasted from the clipboard (`image`, `table` or
    /// `text`). The bytes are checked and written to the data root's
    /// `pasted/` folder, then sniffed like any other file. The file is
    /// deleted when it leaves the selection, and the folder at exit.
    ///
    /// Takes the bytes by reference (or owned, which is dropped on return):
    /// the core makes no copy of them, and writes them with no lock held.
    pub fn add_pasted(
        &self,
        kind: &str,
        bytes: impl AsRef<[u8]>,
    ) -> CommandResult<Vec<FileResult>> {
        let kind = intake::PasteKind::parse(kind)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownPasteKind))?;
        if lock(&self.files).len() >= MAX_FILES {
            return Err(CommandError::with("E_LIMIT_EXCEEDED", Reason::TooManyFiles));
        }
        let pasted = intake::paste(&self.pasted_dir, kind, bytes.as_ref())?;
        drop(bytes);
        let item = Intake {
            display_name: pasted.display_name,
            folder: None,
            path: pasted.path.clone(),
            temp: true,
        };
        match sniff(item) {
            Sniffed::Accepted(entry, result) => {
                let mut files = lock(&self.files);
                if files.len() >= MAX_FILES {
                    drop(files);
                    let _ = std::fs::remove_file(&pasted.path);
                    return Err(CommandError::with("E_LIMIT_EXCEEDED", Reason::TooManyFiles));
                }
                files.insert(entry.descriptor.file_id.clone(), *entry);
                Ok(vec![result])
            }
            Sniffed::Rejected(result) => {
                let _ = std::fs::remove_file(&pasted.path);
                Ok(vec![result])
            }
        }
    }

    pub fn remove_file(&self, file_id: &str) -> CommandResult<()> {
        let entry = lock(&self.files)
            .remove(file_id)
            .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?;
        // A call using the worker right now closes it when it finds the
        // entry gone (`release_file`).
        self.files_idle.notify_all();
        self.close_entry(entry);
        Ok(())
    }

    pub(crate) fn close_entry(&self, mut entry: FileEntry) {
        if let Some(worker) = entry.worker.take() {
            self.close_worker(worker);
        }
        if entry.temp {
            let _ = std::fs::remove_file(&entry.path);
        }
    }

    /// Resolves an ambiguous format (a polyglot) or allows downloading an
    /// online-only file, then sniffs again.
    pub fn resolve_file(
        &self,
        file_id: &str,
        format: Option<&str>,
        allow_download: bool,
    ) -> CommandResult<FileDescriptor> {
        let (path, placeholder) = {
            let files = lock(&self.files);
            let entry = files
                .get(file_id)
                .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?;
            (entry.path.clone(), entry.descriptor.cloud_placeholder)
        };
        // Downloading can take minutes: no lock while the sync client works.
        let fresh = if allow_download && placeholder {
            // Reading the file is what makes the sync client download it; the
            // bytes themselves aren't kept.
            std::fs::File::open(&path)
                .and_then(|mut f| std::io::copy(&mut f, &mut std::io::sink()))
                .map_err(|_| CommandError::with("E_INPUT_CORRUPT", Reason::DownloadFailed))?;
            Some(
                converter_sniffer::sniff_path(&path)
                    .map_err(|_| CommandError::with("E_INPUT_CORRUPT", Reason::Unreadable))?,
            )
        } else {
            None
        };
        let mut files = lock(&self.files);
        let entry = files
            .get_mut(file_id)
            .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?;
        if let Some(fresh) = fresh {
            let id = entry.descriptor.file_id.clone();
            entry.descriptor = FileDescriptor {
                file_id: id,
                display_name: entry.descriptor.display_name.clone(),
                folder: entry.descriptor.folder.clone(),
                ..fresh
            };
        }
        if let Some(name) = format {
            let format = parse_format(name)
                .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownFormat))?;
            let d = &mut entry.descriptor;
            if !d.candidates.contains(&format) {
                return Err(CommandError::with("E_INTERNAL", Reason::NotACandidate));
            }
            d.format = format;
            d.candidates.clear();
            d.actionable = format != Format::Unknown && !d.cloud_placeholder;
        }
        Ok(entry.descriptor.clone())
    }

    /// Keeps a password for this file and checks it by inspecting the file.
    /// Held in memory until the file is removed.
    pub fn submit_password(&self, file_id: &str, password: String) -> CommandResult<InspectInfo> {
        self.refuse_office_password(file_id)?;
        {
            let mut files = lock(&self.files);
            let entry = files
                .get_mut(file_id)
                .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?;
            entry.password = Some(Zeroizing::new(password));
        }
        let result = self.inspect_file(file_id);
        if matches!(&result, Err(e) if e.code == "E_PASSWORD_WRONG") {
            if let Some(entry) = lock(&self.files).get_mut(file_id) {
                entry.password = None;
            }
        }
        result
    }

    /// `E_PASSWORD_REQUIRED` / `officePassword` for a password-protected
    /// Office file, which no password the user types can open.
    pub(crate) fn refuse_office_password(&self, file_id: &str) -> CommandResult<()> {
        let protected = lock(&self.files)
            .get(file_id)
            .is_some_and(|e| is_office_password(&e.descriptor));
        if protected {
            return Err(CommandError::with(
                "E_PASSWORD_REQUIRED",
                Reason::OfficePassword,
            ));
        }
        Ok(())
    }

    /// How many file workers are running (bounded by [`MAX_LIVE_FILE_WORKERS`](crate::MAX_LIVE_FILE_WORKERS)).
    pub fn live_file_workers(&self) -> usize {
        lock(&self.files)
            .values()
            .filter(|e| e.worker.is_some() || e.busy)
            .count()
    }

    /// How many of these files repeat an earlier one byte for byte
    /// (smart suggestions, ADR 0005). Unknown IDs are ignored.
    pub fn duplicates(&self, file_ids: &[String]) -> usize {
        let files: Vec<(u64, PathBuf)> = {
            let files = lock(&self.files);
            file_ids
                .iter()
                .filter_map(|id| files.get(id))
                .map(|e| (e.descriptor.size_bytes, e.path.clone()))
                .collect()
        };
        dupes::count_duplicates(&files)
    }

    pub(crate) fn formats_of(&self, file_ids: &[String]) -> CommandResult<Vec<RouterFormat>> {
        let files = lock(&self.files);
        let mut formats = Vec::new();
        for id in file_ids {
            let entry = files
                .get(id)
                .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?;
            if !entry.descriptor.actionable {
                return Err(CommandError::with("E_INTERNAL", Reason::NotActionable));
            }
            let format = RouterFormat::parse(entry.descriptor.format.as_str())
                .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownFormat))?;
            formats.push(format);
        }
        Ok(formats)
    }
}

/// One file about to be sniffed into the selection.
struct Intake {
    path: PathBuf,
    display_name: String,
    folder: Option<String>,
    /// Lives in `pasted/` and is deleted with the selection entry.
    temp: bool,
}

/// What sniffing one file gave: an entry to insert, or the rejection.
enum Sniffed {
    Accepted(Box<FileEntry>, FileResult),
    Rejected(FileResult),
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Sniffs one file. Touches no shared state, so it needs no lock.
fn sniff(item: Intake) -> Sniffed {
    match converter_sniffer::sniff_path(&item.path) {
        Ok(mut descriptor) => {
            descriptor.display_name = item.display_name.clone();
            descriptor.folder = item.folder;
            if is_office_password(&descriptor) {
                // Shown, but not something to convert or to ask a password for.
                descriptor.actionable = false;
            }
            let mut entry = FileEntry::new(item.path, descriptor.clone());
            entry.temp = item.temp;
            Sniffed::Accepted(
                Box::new(entry),
                FileResult {
                    file: Some(descriptor),
                    error: None,
                    display_name: item.display_name,
                    count: None,
                },
            )
        }
        Err(e) => Sniffed::Rejected(FileResult {
            file: None,
            error: Some(
                serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unreadable".into()),
            ),
            display_name: item.display_name,
            count: None,
        }),
    }
}
