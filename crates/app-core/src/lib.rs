//! The app behind the Tauri commands (spec section 11.1), with no Tauri
//! dependency so it can be tested end to end.
//!
//! The renderer never sees a path or file bytes. It holds `fileId`s (random,
//! from the sniffer) and `jobId`s; everything else stays here. Native
//! dialogs and drag and drop live in the Tauri layer, which hands paths to
//! [`App::add_paths`] and save destinations to [`App::save_outputs`].
//!
//! Two kinds of jobs share the [`JobStore`]:
//! - **User jobs** from [`App::create_job`], run by the scheduler and shown
//!   in the UI.
//! - **File workers**, one per opened file: the file is staged once and a
//!   long-lived sandboxed worker answers `inspect`, `render` and `pageText`.
//!   They're hidden from snapshots and events.

mod config;
mod defaults;
mod dupes;
mod engines;
mod files;
mod intake;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use converter_jobstore::{
    DataRoot, FolderLayout, InputSource, JobError, JobEvent, JobId, JobSnapshot, JobStore,
    SaveCheck, SaveDestination, SaveOptions, Snapshot, StoreConfig, SyncRoots,
};
use converter_protocol::{Codec, SecretString, Secrets};
use converter_router::{
    build_registry, matrix, targets, AdapterEntry, Availability, Catalog, Format as RouterFormat,
    OutputFormat, Plan, Planner, Platform, Registry, Targets,
};
use converter_sandbox::{probe, SandboxConfig, SandboxLauncher};
use converter_scheduler::{pool_for, JobPart, JobSpec, Runner, RunnerConfig, SystemMemory};
use converter_sniffer::{FileDescriptor, Format};
use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

pub use config::find_dev_root;
pub use defaults::{defaults, with_defaults};
pub use engines::Engines;
use files::{FileEntry, FileWorker};
pub use files::{InspectInfo, PageText, Preview};

/// Most files in one selection (blueprint section 6.2 default).
pub const MAX_FILES: usize = 200;
/// Most file workers alive at once. Each is a Python process with the PDF
/// engines loaded (about 50 MB), so a 200-file selection must not start 200.
pub const MAX_LIVE_FILE_WORKERS: usize = 4;
/// Preview resolutions the UI may ask for.
pub const PREVIEW_DPI: &[u32] = &[48, 72, 96, 144];
/// Longest side of a picture put on the clipboard.
pub const CLIPBOARD_SIDE: u32 = 4096;
/// Largest PNG read back for the clipboard.
const MAX_CLIPBOARD_PNG: u64 = 128 * 1024 * 1024;
/// Picture outputs that can be copied as an image.
const COPYABLE_PICTURES: &[Format] = &[
    Format::Png,
    Format::Jpg,
    Format::Webp,
    Format::Gif,
    Format::Bmp,
    Format::Tiff,
    Format::Avif,
    Format::Heic,
    Format::Svg,
    Format::Ico,
];

/// A command failure the UI can show. `code` is from the catalog or a
/// command-level reason; nothing here contains a path or document text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Value>,
}

impl CommandError {
    pub fn new(code: &str, reason: &str) -> Self {
        CommandError {
            code: code.into(),
            reason: Some(reason.into()),
            issues: Vec::new(),
        }
    }

    fn from_job(e: JobError) -> Self {
        let code = match &e {
            JobError::UnknownJob(_) => "E_UNKNOWN_JOB",
            JobError::Busy => "E_BUSY",
            JobError::NeedsInputOverwriteConfirmation => "E_CONFIRM_OVERWRITE",
            JobError::WrongState { .. } => "E_WRONG_STATE",
            _ => e.code().unwrap_or("E_INTERNAL"),
        };
        CommandError {
            code: code.into(),
            reason: None,
            issues: Vec::new(),
        }
    }
}

pub type CommandResult<T> = Result<T, CommandError>;

/// Where everything lives. Built by the Tauri layer from bundle resources
/// in a release, or from the repository in development.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// `None` uses the platform default (spec 10.4).
    pub data_root: Option<PathBuf>,
    /// Holds `engines/engines.yaml` (the approved-engine catalog).
    pub resources: PathBuf,
    pub adapters_dir: PathBuf,
    pub python: PathBuf,
    pub runner: PathBuf,
    /// Arguments before the runner's own (`--sandbox-run` for the app itself).
    pub runner_args: Vec<std::ffi::OsString>,
    pub user_memory_limit: Option<u64>,
    /// Development only: list `devOnly` adapters too.
    pub include_dev_adapters: bool,
}

/// Facts for the Settings screen. No file names or contents (spec 11.1).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub app_version: &'static str,
    pub protocol_version: u32,
    pub adapters: Vec<String>,
    pub sandbox: Option<probe::ProbeReport>,
    pub processing_enabled: bool,
    pub sync_folder: Option<String>,
    /// Engines found on this computer (ADR 0003).
    pub engines: Vec<String>,
    /// `macos`, `windows` or `linux`: which install instructions to show.
    pub platform: &'static str,
}

/// One file as the renderer sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileDescriptor>,
    /// Set when the file was rejected; `displayName` says which.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub display_name: String,
    /// With `error: "folderTruncated"`: how many files of the dropped
    /// folders didn't fit under [`MAX_FILES`] and were left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

type Sink = Arc<dyn Fn(&JobEvent) + Send + Sync>;

pub struct App {
    store: Arc<JobStore>,
    runner: Arc<Runner>,
    launcher: Arc<SandboxLauncher>,
    codec: Arc<Codec>,
    registry: Registry,
    /// What this installation can run: platform and installed engines.
    availability: Availability,
    engines: Engines,
    files: Mutex<BTreeMap<String, FileEntry>>,
    /// File-worker jobs: never shown to the renderer.
    hidden: Arc<Mutex<HashSet<JobId>>>,
    /// Workers for previewing finished jobs' outputs, by job.
    output_workers: Mutex<BTreeMap<JobId, FileWorker>>,
    /// Per job, per staged input: the dropped folder it came from
    /// (`root/relative/dir`), for keeping the layout when saving.
    layouts: Mutex<BTreeMap<JobId, Vec<Option<String>>>>,
    /// Pasted items (`<data root>/pasted`), cleared at start and exit.
    pasted_dir: PathBuf,
    sandbox: Mutex<Option<probe::ProbeReport>>,
    preview_dirs: Arc<dyn Fn(PathBuf) -> String + Send + Sync>,
    revoke_preview: Arc<dyn Fn(&str) + Send + Sync>,
}

impl App {
    /// Opens the store (collecting dead jobs), loads the adapter registry
    /// and prepares the sandbox. Call [`App::self_test`] before processing.
    ///
    /// `on_event` receives user-job events; `register_preview` maps a
    /// directory of `<n>.png` files to a URL token, `revoke_preview` drops one.
    pub fn open(
        config: &AppConfig,
        on_event: Sink,
        register_preview: Arc<dyn Fn(PathBuf) -> String + Send + Sync>,
        revoke_preview: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> Result<App, String> {
        let sync = SyncRoots::discover();
        let root = match &config.data_root {
            Some(p) => DataRoot::open(p, &sync),
            None => DataRoot::open_default(&sync),
        }
        .map_err(|e| format!("data folder: {e}"))?;

        let hidden: Arc<Mutex<HashSet<JobId>>> = Arc::default();
        let sink: Arc<RwLock<Sink>> = Arc::new(RwLock::new(on_event));
        let filter = {
            let hidden = hidden.clone();
            let sink = sink.clone();
            Arc::new(move |e: &JobEvent| {
                if lock(&hidden).contains(&e.job_id) {
                    return;
                }
                let f = sink.read().unwrap_or_else(|p| p.into_inner()).clone();
                f(e);
            })
        };
        let (store, _sweep) = JobStore::open(
            root,
            sync,
            StoreConfig {
                on_event: Some(filter),
                ..StoreConfig::default()
            },
        )
        .map_err(|e| format!("job store: {e}"))?;
        let store = Arc::new(store);
        let pasted_dir = store.data_root().path().join(intake::PASTED_DIR);
        intake::clear_pasted(&pasted_dir);

        let catalog = Catalog::load(&config.resources).map_err(problems)?;
        let registry = build_registry(&config.adapters_dir, &catalog, config.include_dev_adapters)
            .map_err(problems)?;
        let mut sandbox_config =
            SandboxConfig::discover(&config.python, &config.adapters_dir, &config.runner)?;
        sandbox_config.runner_args = config.runner_args.clone();
        let engines = Engines::discover(&config.python);
        sandbox_config.grants = engines.adapter_grants(&registry);
        let availability = Availability {
            platform: Platform::current(),
            engines: Some(engines.installed.clone()),
        };
        let launcher = Arc::new(SandboxLauncher::new(sandbox_config));
        let runner = Runner::new(
            store.clone(),
            launcher.clone(),
            Arc::new(SystemMemory),
            RunnerConfig {
                user_memory_limit: config.user_memory_limit,
                ..RunnerConfig::default()
            },
        );
        Ok(App {
            store,
            runner,
            launcher,
            codec: Arc::new(Codec::new()),
            registry,
            availability,
            engines,
            files: Mutex::default(),
            hidden,
            output_workers: Mutex::default(),
            layouts: Mutex::default(),
            pasted_dir,
            sandbox: Mutex::new(None),
            preview_dirs: register_preview,
            revoke_preview,
        })
    }

    // ------------------------------------------------------------- sandbox

    /// Runs the sandbox probe (spec 9.3). Processing stays disabled until it
    /// passes.
    pub fn self_test(&self, home: &Path) -> probe::ProbeReport {
        let scratch = self.store.data_root().path().join("selftest");
        // A probe that couldn't run says why, for diagnostics and bug reports.
        let report = probe::self_test(&self.launcher, &scratch, home).unwrap_or_else(|e| {
            probe::ProbeReport {
                attempts: vec![probe::Attempt {
                    name: "probe_ran".into(),
                    expected: "allowed",
                    outcome: e,
                    passed: false,
                }],
                passed: false,
            }
        });
        *lock(&self.sandbox) = Some(report.clone());
        report
    }

    fn require_sandbox(&self) -> CommandResult<()> {
        match &*lock(&self.sandbox) {
            Some(r) if r.passed => Ok(()),
            _ => Err(CommandError::new("E_SANDBOX_UNAVAILABLE", "selfTestFailed")),
        }
    }

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
        let mut files = lock(&self.files);
        let room = MAX_FILES.saturating_sub(files.len());
        if !has_folder && items.len() > room {
            return Err(CommandError::new("E_LIMIT_EXCEEDED", "tooManyFiles"));
        }
        let left_out = items.len().saturating_sub(room);
        items.truncate(room);
        for item in items {
            results.push(sniff_into(&mut files, item));
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
    pub fn add_pasted(&self, kind: &str, bytes: Vec<u8>) -> CommandResult<Vec<FileResult>> {
        let kind = intake::PasteKind::parse(kind)
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownPasteKind"))?;
        let mut files = lock(&self.files);
        if files.len() >= MAX_FILES {
            return Err(CommandError::new("E_LIMIT_EXCEEDED", "tooManyFiles"));
        }
        let pasted = intake::paste(&self.pasted_dir, kind, &bytes)?;
        drop(bytes);
        let result = sniff_into(
            &mut files,
            Intake {
                display_name: pasted.display_name,
                folder: None,
                path: pasted.path.clone(),
                temp: true,
            },
        );
        if result.file.is_none() {
            let _ = std::fs::remove_file(&pasted.path);
        }
        Ok(vec![result])
    }

    pub fn remove_file(&self, file_id: &str) -> CommandResult<()> {
        let entry = lock(&self.files)
            .remove(file_id)
            .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
        self.close_entry(entry);
        Ok(())
    }

    fn close_entry(&self, mut entry: FileEntry) {
        if let Some(worker) = entry.worker.take() {
            self.close_worker(worker);
        }
        if entry.temp {
            let _ = std::fs::remove_file(&entry.path);
        }
    }

    fn close_worker(&self, worker: FileWorker) {
        for token in worker.tokens.values() {
            (self.revoke_preview)(token);
        }
        let job = worker.job.clone();
        worker.shutdown();
        let _ = self.store.fail(
            &job,
            converter_jobstore::JobFailure {
                code: "E_CANCELLED".into(),
                retryable: false,
            },
        );
        let _ = self.store.discard(&job);
        lock(&self.hidden).remove(&job);
    }

    /// Resolves an ambiguous format (a polyglot) or allows downloading an
    /// online-only file, then sniffs again.
    pub fn resolve_file(
        &self,
        file_id: &str,
        format: Option<&str>,
        allow_download: bool,
    ) -> CommandResult<FileDescriptor> {
        let mut files = lock(&self.files);
        let entry = files
            .get_mut(file_id)
            .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
        if allow_download && entry.descriptor.cloud_placeholder {
            // Reading the file is what makes the sync client download it.
            std::fs::read(&entry.path)
                .map_err(|_| CommandError::new("E_INPUT_CORRUPT", "downloadFailed"))?;
            let fresh = converter_sniffer::sniff_path(&entry.path)
                .map_err(|_| CommandError::new("E_INPUT_CORRUPT", "unreadable"))?;
            let id = entry.descriptor.file_id.clone();
            entry.descriptor = FileDescriptor {
                file_id: id,
                folder: entry.descriptor.folder.clone(),
                ..fresh
            };
        }
        if let Some(name) = format {
            let format = parse_format(name)
                .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownFormat"))?;
            let d = &mut entry.descriptor;
            if !d.candidates.contains(&format) {
                return Err(CommandError::new("E_INTERNAL", "notACandidate"));
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
        {
            let mut files = lock(&self.files);
            let entry = files
                .get_mut(file_id)
                .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
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

    /// How many file workers are running (bounded by [`MAX_LIVE_FILE_WORKERS`]).
    pub fn live_file_workers(&self) -> usize {
        lock(&self.files)
            .values()
            .filter(|e| e.worker.is_some())
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

    /// Page count, encryption and warnings, from the file's sandboxed worker.
    pub fn inspect_file(&self, file_id: &str) -> CommandResult<InspectInfo> {
        self.require_sandbox()?;
        if self.file_adapter(file_id)?.is_none() {
            return Ok(InspectInfo::unknown());
        }
        self.with_file_worker(file_id, |w, secrets| w.inspect(secrets))
    }

    /// The available adapter that inspects and previews this file, if any.
    fn file_adapter(&self, file_id: &str) -> CommandResult<Option<&'static str>> {
        let format = lock(&self.files)
            .get(file_id)
            .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?
            .descriptor
            .format;
        Ok(self.usable_preview_adapter(format))
    }

    fn usable_preview_adapter(&self, format: Format) -> Option<&'static str> {
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
            return Err(CommandError::new("E_INTERNAL", "unsupportedDpi"));
        }
        let register = self.preview_dirs.clone();
        if let Some((job, output)) = parse_output_ref(reference) {
            return self.with_output_worker(&job, output, |w, input| {
                w.render(input, page, dpi, &*register)
            });
        }
        if self.file_adapter(reference)?.is_none() {
            return Err(CommandError::new("E_INTERNAL", "notPreviewable"));
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
                .with_output_worker(&job, output, |w, input| w.page_text(input, None, page));
        }
        self.with_file_worker(reference, |w, secrets| {
            let input = w.input.clone();
            w.page_text(&input, secrets, page)
        })
    }

    fn with_file_worker<T>(
        &self,
        file_id: &str,
        f: impl FnOnce(&mut FileWorker, Option<&Secrets>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let mut files = lock(&self.files);
        let needs_worker = files
            .get(file_id)
            .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?
            .worker
            .is_none();
        if needs_worker {
            // Close the least recently used workers to stay under the cap.
            let mut live: Vec<(std::time::Instant, String)> = files
                .iter()
                .filter(|(id, e)| e.worker.is_some() && id.as_str() != file_id)
                .map(|(id, e)| (e.last_used, id.clone()))
                .collect();
            live.sort();
            let excess = (live.len() + 1).saturating_sub(MAX_LIVE_FILE_WORKERS);
            for (_, id) in live.into_iter().take(excess) {
                if let Some(w) = files.get_mut(&id).and_then(|e| e.worker.take()) {
                    self.close_worker(w);
                }
            }
        }
        let entry = files.get_mut(file_id).expect("checked above");
        entry.last_used = std::time::Instant::now();
        if !entry.descriptor.actionable {
            return Err(CommandError::new("E_INTERNAL", "notActionable"));
        }
        if entry.worker.is_none() {
            let format = entry.descriptor.format;
            let adapter = self
                .usable_preview_adapter(format)
                .ok_or_else(|| CommandError::new("E_INTERNAL", "notPreviewable"))?;
            let (job, _) = self.store.create_job().map_err(CommandError::from_job)?;
            lock(&self.hidden).insert(job.clone());
            self.store
                .stage_inputs(
                    &job,
                    &[InputSource {
                        path: entry.path.clone(),
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
            entry.worker = Some(FileWorker::start(
                &self.launcher,
                self.codec.clone(),
                adapter,
                job,
                dir,
                format!("in/0.{}", format.as_str()),
                entry.descriptor.size_bytes,
            )?);
        }
        let secrets = entry.secrets();
        let worker = entry.worker.as_mut().expect("started above");
        let result = f(worker, secrets.as_ref());
        if matches!(&result, Err(e) if e.code == "E_PROTOCOL" || e.code == "E_TIMEOUT" || e.code == "E_INSTALL_CORRUPT")
        {
            // The worker is gone; start a fresh one next time.
            if let Some(w) = entry.worker.take() {
                drop(files);
                self.close_worker(w);
            }
        }
        result
    }

    fn with_output_worker<T>(
        &self,
        job: &JobId,
        output: usize,
        f: impl FnOnce(&mut FileWorker, &str) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let snapshot = self.job_snapshot(job)?;
        let out = snapshot
            .outputs
            .get(output)
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownOutput"))?;
        let adapter = parse_format(&out.extension)
            .and_then(|f| self.usable_preview_adapter(f))
            .ok_or_else(|| CommandError::new("E_INTERNAL", "notPreviewable"))?;
        let input = format!("out/{}", out.path);
        let mut workers = lock(&self.output_workers);
        if !workers.contains_key(job) {
            let dir = self
                .store
                .job_dirs(job)
                .map_err(CommandError::from_job)?
                .root;
            let worker = FileWorker::start(
                &self.launcher,
                self.codec.clone(),
                adapter,
                job.clone(),
                dir,
                input.clone(),
                out.size_bytes,
            )?;
            workers.insert(job.clone(), worker);
        }
        let worker = workers.get_mut(job).expect("inserted above");
        f(worker, &input)
    }

    fn close_output_worker(&self, job: &JobId) {
        if let Some(worker) = lock(&self.output_workers).remove(job) {
            for token in worker.tokens.values() {
                (self.revoke_preview)(token);
            }
            worker.shutdown();
        }
    }

    // ------------------------------------------------------------- targets

    /// Operations and conversions for these files (spec 11.1 `get_targets`).
    pub fn get_targets(&self, file_ids: &[String]) -> CommandResult<Targets> {
        let formats = self.formats_of(file_ids)?;
        let mut t = targets(&self.registry, &self.availability, &formats);
        // Every offered conversion runs as one job in one adapter (ADR 0003).
        for g in &mut t.groups {
            g.conversions
                .retain(|c| c.plan.as_ref().is_none_or(|p| p.hops.len() == 1));
        }
        // A shared target ("all to PDF") needs that one-job route in every
        // group. The groups may use different adapters: each becomes a part
        // of the job.
        let groups = t.groups.clone();
        t.shared.retain(|s| {
            groups.iter().all(|g| {
                g.conversions
                    .iter()
                    .any(|c| c.to == s.to && c.plan.is_some() && c.unavailable.is_none())
            })
        });
        Ok(t)
    }

    fn formats_of(&self, file_ids: &[String]) -> CommandResult<Vec<RouterFormat>> {
        let files = lock(&self.files);
        let mut formats = Vec::new();
        for id in file_ids {
            let entry = files
                .get(id)
                .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
            if !entry.descriptor.actionable {
                return Err(CommandError::new("E_INTERNAL", "notActionable"));
            }
            let format = RouterFormat::parse(entry.descriptor.format.as_str())
                .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownFormat"))?;
            formats.push(format);
        }
        Ok(formats)
    }

    /// Every adapter's manifest and parameter schema, for generated forms.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    // ---------------------------------------------------------------- jobs

    /// Validates params, stages inputs and starts an operation (spec 11.1).
    pub fn create_job(
        &self,
        operation_id: &str,
        file_ids: &[String],
        params: Value,
    ) -> CommandResult<JobId> {
        self.require_sandbox()?;
        let entry = self
            .registry
            .get(operation_id)
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownOperation"))?;
        let (inputs, outputs) = match (&entry.manifest.inputs, &entry.manifest.outputs) {
            (Some(i), Some(o)) => (i, o),
            _ => return Err(CommandError::new("E_INTERNAL", "notAnOperation")),
        };
        if self.availability_of(entry).is_err() {
            return Err(CommandError::new("E_INSTALL_CORRUPT", "engineMissing"));
        }
        let fixed = match outputs.format {
            OutputFormat::Fixed(f) => Some(f),
            OutputFormat::Chosen => Some(
                params["to"]
                    .as_str()
                    .and_then(RouterFormat::parse)
                    .ok_or_else(|| CommandError::new("E_PARAMS_INVALID", "unknownFormat"))?,
            ),
            OutputFormat::Same => None,
        };
        if let Some(format) = fixed {
            let output_format = parse_format(format.as_str())
                .ok_or_else(|| CommandError::new("E_INSTALL_CORRUPT", "unknownOutputFormat"))?;
            let mut part = self.prepare(
                entry,
                file_ids,
                params,
                &|f| inputs.formats.contains(&f),
                Some((inputs.min, inputs.max)),
            )?;
            part.part.output_format = output_format;
            return self.start(vec![part]);
        }

        // Each file keeps its format (ADR 0004): one part per format, in the
        // order the formats first appear.
        let count = file_ids.len() as u32;
        if count < inputs.min || count > inputs.max {
            return Err(CommandError::new("E_INTERNAL", "wrongFileCount"));
        }
        let formats = self.formats_of(file_ids)?;
        let mut groups: Vec<(RouterFormat, Vec<String>)> = Vec::new();
        for (id, &f) in file_ids.iter().zip(&formats) {
            match groups.iter_mut().find(|g| g.0 == f) {
                Some(g) => g.1.push(id.clone()),
                None => groups.push((f, vec![id.clone()])),
            }
        }
        let mut parts = Vec::with_capacity(groups.len());
        for (format, ids) in &groups {
            let mut part = self.prepare(
                entry,
                ids,
                params.clone(),
                &|f| f == *format && inputs.formats.contains(&f),
                None,
            )?;
            part.part.output_format = parse_format(format.as_str())
                .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownFormat"))?;
            parts.push(part);
        }
        self.start(parts)
    }

    /// Converts every file to `to`, in one job (ADR 0003). Files that need
    /// different adapters (Word files and photos to PDF) run as one part per
    /// adapter; each part gets the params its schema knows, so a mixed
    /// selection runs with every adapter's defaults.
    ///
    /// With `"skipSame": true` in `params`, files already in the target
    /// format are left out; see [`App::create_conversion_counted`].
    pub fn create_conversion(
        &self,
        file_ids: &[String],
        to: &str,
        params: Value,
    ) -> CommandResult<JobId> {
        self.create_conversion_counted(file_ids, to, params)
            .map(|(job, _)| job)
    }

    /// [`App::create_conversion`], also returning how many files `skipSame`
    /// left out. If every file is skipped there is nothing to run and the
    /// error is `E_PARAMS_INVALID` / `allSkipped`.
    pub fn create_conversion_counted(
        &self,
        file_ids: &[String],
        to: &str,
        mut params: Value,
    ) -> CommandResult<(JobId, usize)> {
        self.require_sandbox()?;
        let target = RouterFormat::parse(to)
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownFormat"))?;
        let skip_same = params
            .as_object_mut()
            .and_then(|m| m.remove("skipSame"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut formats = self.formats_of(file_ids)?;
        let mut file_ids = file_ids.to_vec();
        let mut skipped = 0;
        if skip_same {
            let keep: Vec<bool> = formats.iter().map(|&f| f != target).collect();
            skipped = keep.iter().filter(|k| !**k).count();
            let mut it = keep.iter();
            file_ids.retain(|_| *it.next().unwrap_or(&true));
            let mut it = keep.iter();
            formats.retain(|_| *it.next().unwrap_or(&true));
            if file_ids.is_empty() {
                return Err(CommandError::new("E_PARAMS_INVALID", "allSkipped"));
            }
        }
        let file_ids = &file_ids[..];
        let planner = Planner::new(&self.registry, &self.availability);
        let mut adapters: BTreeMap<RouterFormat, String> = BTreeMap::new();
        // (adapter, its files, their formats), in the order files were given.
        let mut groups: Vec<(String, Vec<String>, BTreeSet<RouterFormat>)> = Vec::new();
        for (id, &from) in file_ids.iter().zip(&formats) {
            let adapter = match adapters.get(&from) {
                Some(a) => a.clone(),
                None => {
                    let plan = best_plan(&planner, from, target)
                        .ok_or_else(|| CommandError::new("E_INTERNAL", "noRoute"))?;
                    if plan.hops.len() != 1 {
                        return Err(CommandError::new("E_INTERNAL", "multiHop"));
                    }
                    let a = plan.hops[0].adapter.clone();
                    adapters.insert(from, a.clone());
                    a
                }
            };
            match groups.iter_mut().find(|g| g.0 == adapter) {
                Some(g) => {
                    g.1.push(id.clone());
                    g.2.insert(from);
                }
                None => groups.push((adapter, vec![id.clone()], BTreeSet::from([from]))),
            }
        }
        if groups.is_empty() {
            return Err(CommandError::new("E_INTERNAL", "noFiles"));
        }
        let Value::Object(given) = params else {
            return Err(CommandError::new("E_PARAMS_INVALID", "notAnObject"));
        };
        let output_format = parse_format(target.as_str())
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownFormat"))?;
        let mixed = groups.len() > 1;
        let mut parts = Vec::with_capacity(groups.len());
        for (adapter, ids, sources) in &groups {
            let entry = self
                .registry
                .get(adapter)
                .ok_or_else(|| CommandError::new("E_INSTALL_CORRUPT", "unknownAdapter"))?;
            let known = &entry.params_schema["properties"];
            let mut map: serde_json::Map<String, Value> = given
                .iter()
                .filter(|(k, _)| !mixed || known.get(k.as_str()).is_some())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            map.insert("to".into(), json!(target.as_str()));
            let mut part = self.prepare(
                entry,
                ids,
                Value::Object(map),
                &|f| sources.contains(&f),
                None,
            )?;
            part.part.output_format = output_format;
            parts.push(part);
        }
        Ok((self.start(parts)?, skipped))
    }

    fn availability_of(&self, entry: &AdapterEntry) -> Result<(), ()> {
        let installed = self.availability.engines.as_ref();
        let ok = entry
            .manifest
            .platforms
            .contains(&self.availability.platform)
            && entry
                .manifest
                .requires
                .iter()
                .all(|e| installed.is_none_or(|i| i.contains(e)));
        if ok {
            Ok(())
        } else {
            Err(())
        }
    }

    /// Validates params, orders the files and gathers what a job part needs.
    fn prepare(
        &self,
        entry: &AdapterEntry,
        file_ids: &[String],
        params: Value,
        accepts: &dyn Fn(RouterFormat) -> bool,
        count_limits: Option<(u32, u32)>,
    ) -> CommandResult<Prepared> {
        let manifest = &entry.manifest;
        let (mut params, secrets) = split_secrets(params)?;
        let validator = jsonschema::validator_for(&entry.params_schema)
            .map_err(|_| CommandError::new("E_INSTALL_CORRUPT", "badParamsSchema"))?;
        let issues: Vec<Value> = validator
            .iter_errors(&params)
            .take(32)
            .map(|e| json!({ "path": e.instance_path().to_string() }))
            .collect();
        if !issues.is_empty() {
            return Err(CommandError {
                code: "E_PARAMS_INVALID".into(),
                reason: None,
                issues,
            });
        }

        let ordered = order_files(&params, file_ids)?;
        let count = ordered.len() as u32;
        if let Some((min, max)) = count_limits {
            if count < min || count > max {
                return Err(CommandError::new("E_INTERNAL", "wrongFileCount"));
            }
        }
        if count == 0 {
            return Err(CommandError::new("E_INTERNAL", "wrongFileCount"));
        }

        let (sources, folders, password, titles) = {
            let files = lock(&self.files);
            let mut sources = Vec::new();
            let mut folders = Vec::new();
            let mut password: Option<SecretString> = None;
            let mut titles = Vec::new();
            for id in &ordered {
                let entry = files
                    .get(id)
                    .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
                let format = RouterFormat::parse(entry.descriptor.format.as_str());
                if !entry.descriptor.actionable || !format.is_some_and(accepts) {
                    return Err(CommandError::new("E_INTERNAL", "unsupportedInput"));
                }
                if password.is_none() {
                    password = entry
                        .password
                        .as_ref()
                        .map(|p| SecretString::new(p.to_string()));
                }
                titles.push(stem(&entry.descriptor.display_name));
                folders.push(entry.descriptor.folder.clone());
                sources.push(InputSource {
                    path: entry.path.clone(),
                    extension: entry.descriptor.format.as_str().to_owned(),
                });
            }
            (sources, folders, password, titles)
        };

        // The core resolves file IDs; the worker only ever sees input order.
        let wants_titles = entry.params_schema["properties"]
            .get("fileTitles")
            .is_some();
        if let Some(obj) = params.as_object_mut() {
            obj.remove("order");
            if wants_titles && !obj.contains_key("fileTitles") {
                obj.insert("fileTitles".into(), json!(titles));
            }
        }
        let mut secrets = secrets.unwrap_or_default();
        if secrets.password.is_none() {
            secrets.password = password;
        }
        let has_secrets = secrets.password.is_some()
            || secrets.user_password.is_some()
            || secrets.owner_password.is_some();
        Ok(Prepared {
            pool: pool_for(&manifest.id, worker_class(manifest)),
            disk_factor: manifest.limits.disk_factor,
            part: JobPart {
                adapter_id: manifest.id.clone(),
                inputs: Vec::new(),
                timeout_base_s: manifest.limits.timeout_base_s,
                timeout_s_per_mb: manifest.limits.timeout_s_per_mb,
                no_progress_s: manifest.limits.no_progress_s,
                params,
                secrets: has_secrets.then_some(secrets),
                // Set by the caller, which knows the target.
                output_format: Format::Pdf,
            },
            sources,
            folders,
        })
    }

    /// Stages every part's inputs in one job and hands it to the scheduler.
    fn start(&self, parts: Vec<Prepared>) -> CommandResult<JobId> {
        let disk_factor = parts.iter().map(|p| p.disk_factor).fold(0.0, f64::max);
        let pool = strictest_pool(parts.iter().map(|p| p.pool.as_str()));
        let mut sources = Vec::new();
        let mut folders = Vec::new();
        let mut job_parts = Vec::with_capacity(parts.len());
        for p in parts {
            let mut part = p.part;
            part.inputs = (sources.len()..sources.len() + p.sources.len()).collect();
            sources.extend(p.sources);
            folders.extend(p.folders);
            job_parts.push(part);
        }

        let (job, _) = self.store.create_job().map_err(CommandError::from_job)?;
        if let Err(e) = self.store.stage_inputs(&job, &sources, disk_factor) {
            let failure = converter_jobstore::JobFailure {
                code: e.code().unwrap_or("E_INTERNAL").into(),
                retryable: false,
            };
            let _ = self.store.fail(&job, failure);
            return Err(CommandError::from_job(e));
        }
        if folders.iter().any(Option::is_some) {
            lock(&self.layouts).insert(job.clone(), folders);
        }
        self.runner.start(JobSpec {
            job: job.clone(),
            pool,
            parts: job_parts,
        });
        Ok(job)
    }

    pub fn cancel_job(&self, job: &JobId) -> CommandResult<bool> {
        Ok(self.runner.cancel(job))
    }

    pub fn discard_job(&self, job: &JobId) -> CommandResult<()> {
        self.close_output_worker(job);
        lock(&self.layouts).remove(job);
        self.store.discard(job).map_err(CommandError::from_job)
    }

    pub fn check_save(&self, job: &JobId, dest: &SaveDestination) -> CommandResult<SaveCheck> {
        self.store
            .check_save(job, dest)
            .map_err(CommandError::from_job)
    }

    /// Saves outputs to a destination from the native dialog, then deletes the job.
    pub fn save_outputs(
        &self,
        job: &JobId,
        dest: &SaveDestination,
        confirmed_input_overwrite: bool,
    ) -> CommandResult<usize> {
        self.close_output_worker(job);
        let saved = self
            .store
            .save_outputs(
                job,
                dest,
                SaveOptions {
                    confirmed_input_overwrite,
                },
            )
            .map(|r| r.saved.len())
            .map_err(CommandError::from_job)?;
        lock(&self.layouts).remove(job);
        Ok(saved)
    }

    /// Whether saving this job's outputs can keep the dropped folder's
    /// layout: at least one output came from a file that was in a folder.
    pub fn can_keep_layout(&self, job: &JobId) -> bool {
        let Ok(snap) = self.job_snapshot(job) else {
            return false;
        };
        let layouts = lock(&self.layouts);
        let Some(inputs) = layouts.get(job) else {
            return false;
        };
        snap.outputs.iter().any(|o| {
            o.source
                .and_then(|s| inputs.get(s))
                .is_some_and(Option::is_some)
        })
    }

    /// The destination for saving several outputs into `dir`: with the
    /// dropped folder's layout when asked for and possible.
    pub fn folder_destination(
        &self,
        job: &JobId,
        dir: PathBuf,
        keep_layout: bool,
    ) -> SaveDestination {
        if keep_layout && self.can_keep_layout(job) {
            if let Some(inputs) = lock(&self.layouts).get(job).cloned() {
                return SaveDestination::FolderKeepingLayout(FolderLayout { dir, inputs });
            }
        }
        SaveDestination::Folder(dir)
    }

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
        use std::io::Read;
        let snap = self.job_snapshot(job)?;
        if !matches!(
            snap.state,
            converter_jobstore::JobState::Succeeded | converter_jobstore::JobState::Partial
        ) {
            return Err(CommandError::new("E_WRONG_STATE", "jobNotFinished"));
        }
        let out = snap
            .outputs
            .get(index)
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownOutput"))?;
        if out.size_bytes > max_bytes {
            return Err(CommandError::new("E_LIMIT_EXCEEDED", "tooLargeToCopy"));
        }
        let path = self
            .store
            .job_dirs(job)
            .map_err(CommandError::from_job)?
            .out
            .join(&out.path);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| CommandError::new("E_INTERNAL", "unknownOutput"))?;
        if !meta.is_file() {
            return Err(CommandError::new("E_INTERNAL", "unknownOutput"));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|f| f.take(max_bytes + 1).read_to_end(&mut bytes))
            .map_err(|_| CommandError::new("E_INTERNAL", "unreadableOutput"))?;
        if bytes.len() as u64 > max_bytes {
            return Err(CommandError::new("E_LIMIT_EXCEEDED", "tooLargeToCopy"));
        }
        Ok((out.extension.clone(), bytes))
    }

    /// A picture output as PNG bytes for the clipboard. The output is
    /// decoded by its own sandboxed worker (the preview path), which writes
    /// a PNG at most [`CLIPBOARD_SIDE`] pixels on its longest side; the
    /// core only reads that PNG back.
    pub fn clipboard_png(&self, job: &JobId, index: usize) -> CommandResult<Vec<u8>> {
        use std::io::Read;
        self.require_sandbox()?;
        let snap = self.job_snapshot(job)?;
        if !matches!(
            snap.state,
            converter_jobstore::JobState::Succeeded | converter_jobstore::JobState::Partial
        ) {
            return Err(CommandError::new("E_WRONG_STATE", "jobNotFinished"));
        }
        let format = snap
            .outputs
            .get(index)
            .and_then(|o| parse_format(&o.extension))
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownOutput"))?;
        if !COPYABLE_PICTURES.contains(&format) {
            return Err(CommandError::new("E_INTERNAL", "notCopyable"));
        }
        let path = self.with_output_worker(job, index, |w, input| {
            w.render_file(input, CLIPBOARD_SIDE / 8)
        })?;
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&path)
            .and_then(|f| f.take(MAX_CLIPBOARD_PNG + 1).read_to_end(&mut bytes));
        let _ = std::fs::remove_file(&path);
        read.map_err(|_| CommandError::new("E_INTERNAL", "unreadableOutput"))?;
        if bytes.len() as u64 > MAX_CLIPBOARD_PNG {
            return Err(CommandError::new("E_LIMIT_EXCEEDED", "tooLargeToCopy"));
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
            .ok_or_else(|| CommandError::new("E_INTERNAL", "noOutputs"))?;
        Ok(converter_jobstore::sanitize_file_name(
            &out.suggested_name,
            &out.extension,
            "output",
        ))
    }

    fn job_snapshot(&self, job: &JobId) -> CommandResult<JobSnapshot> {
        self.store
            .snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == job)
            .ok_or_else(|| CommandError::new("E_UNKNOWN_JOB", "unknownJob"))
    }

    /// Current state and latest `seq`, for reload recovery (spec 11.2).
    pub fn get_snapshot(&self) -> Snapshot {
        let hidden = lock(&self.hidden).clone();
        let mut snap = self.store.snapshot();
        snap.jobs.retain(|j| !hidden.contains(&j.job_id));
        snap
    }

    pub fn get_diagnostics(&self) -> Diagnostics {
        let sandbox = lock(&self.sandbox).clone();
        Diagnostics {
            app_version: env!("CARGO_PKG_VERSION"),
            protocol_version: converter_protocol::PROTOCOL_VERSION,
            adapters: self
                .registry
                .adapters
                .iter()
                .map(|a| a.manifest.id.clone())
                .collect(),
            processing_enabled: sandbox.as_ref().is_some_and(|r| r.passed),
            sandbox,
            sync_folder: None,
            engines: self.engines.installed.iter().cloned().collect(),
            platform: std::env::consts::OS,
        }
    }

    /// App exit: stop workers, then delete every job directory (spec 10.5).
    pub fn shutdown(&self) {
        let entries: Vec<FileEntry> = std::mem::take(&mut *lock(&self.files))
            .into_values()
            .collect();
        for entry in entries {
            self.close_entry(entry);
        }
        let jobs: Vec<JobId> = lock(&self.output_workers).keys().cloned().collect();
        for job in jobs {
            self.close_output_worker(&job);
        }
        for job in self.store.snapshot().jobs {
            self.runner.cancel(&job.job_id);
        }
        intake::clear_pasted(&self.pasted_dir);
        self.store.shutdown();
    }
}

/// `pdf-protect` collects passwords in password widgets; they must travel
/// as secrets, never params (spec 7.3).
fn split_secrets(params: Value) -> CommandResult<(Value, Option<Secrets>)> {
    let Value::Object(mut map) = params else {
        return Err(CommandError::new("E_PARAMS_INVALID", "notAnObject"));
    };
    let take = |map: &mut serde_json::Map<String, Value>,
                key: &str|
     -> CommandResult<Option<SecretString>> {
        match map.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if !s.is_empty() => Ok(Some(SecretString::new(s))),
            Some(Value::String(_)) => Ok(None),
            Some(_) => Err(CommandError::new("E_PARAMS_INVALID", "passwordNotText")),
        }
    };
    let user = take(&mut map, "userPassword")?;
    let owner = take(&mut map, "ownerPassword")?;
    let secrets = (user.is_some() || owner.is_some()).then(|| Secrets {
        password: None,
        user_password: user,
        owner_password: owner,
    });
    Ok((Value::Object(map), secrets))
}

/// Input order: the `order` param (file IDs from the `fileOrder` widget)
/// when given, otherwise selection order.
fn order_files(params: &Value, file_ids: &[String]) -> CommandResult<Vec<String>> {
    let Some(order) = params.get("order").and_then(Value::as_array) else {
        return Ok(file_ids.to_vec());
    };
    let order: Vec<String> = order
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let mut a = order.clone();
    let mut b = file_ids.to_vec();
    a.sort();
    b.sort();
    if a != b {
        return Err(CommandError::new("E_PARAMS_INVALID", "orderMismatch"));
    }
    Ok(order)
}

/// `<jobId>:<n>` refers to output `n` of a finished job.
fn parse_output_ref(reference: &str) -> Option<(JobId, usize)> {
    let (job, n) = reference.split_once(':')?;
    Some((JobId::parse(job)?, n.parse().ok()?))
}

/// One part of a job before its inputs are staged.
struct Prepared {
    part: JobPart,
    sources: Vec<InputSource>,
    /// The dropped folder of each source, in the same order.
    folders: Vec<Option<String>>,
    pool: String,
    disk_factor: f64,
}

/// A job in parts queues in the pool of its heaviest part: one that runs
/// LibreOffice or FFmpeg waits for that pool's single slot.
fn strictest_pool<'a>(pools: impl Iterator<Item = &'a str>) -> String {
    const ORDER: [&str; 7] = ["office", "media", "redact", "ocr", "render", "image", "pdf"];
    pools
        .min_by_key(|p| ORDER.iter().position(|o| o == p).unwrap_or(ORDER.len()))
        .unwrap_or("pdf")
        .to_owned()
}

fn worker_class(m: &converter_router::Manifest) -> &'static str {
    use converter_router::manifest::WorkerClass;
    match m.worker_class {
        WorkerClass::Pdf => "pdf",
        WorkerClass::Image => "image",
        WorkerClass::Render => "render",
        WorkerClass::Office => "office",
        WorkerClass::Ocr => "ocr",
        WorkerClass::Media => "media",
    }
}

/// The plan the UI offered for `from → to`: the matrix's shape for the
/// blueprint's formats, a direct edge otherwise (ADR 0003).
fn best_plan(planner: &Planner, from: RouterFormat, to: RouterFormat) -> Option<Plan> {
    if from.in_blueprint() && to.in_blueprint() {
        matrix::offered(from, to).and_then(|cell| planner.best_for_cell(from, to, cell))
    } else {
        planner.best_direct(from, to)
    }
}

/// The adapter whose worker inspects and previews files of this format.
fn preview_adapter(format: Format) -> Option<&'static str> {
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

/// One file about to be sniffed into the selection.
struct Intake {
    path: PathBuf,
    display_name: String,
    folder: Option<String>,
    /// Lives in `pasted/` and is deleted with the selection entry.
    temp: bool,
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Sniffs one file and adds it to the selection if it was accepted.
fn sniff_into(files: &mut BTreeMap<String, FileEntry>, item: Intake) -> FileResult {
    match converter_sniffer::sniff_path(&item.path) {
        Ok(mut descriptor) => {
            descriptor.display_name = item.display_name.clone();
            descriptor.folder = item.folder;
            let mut entry = FileEntry::new(item.path, descriptor.clone());
            entry.temp = item.temp;
            files.insert(descriptor.file_id.clone(), entry);
            FileResult {
                file: Some(descriptor),
                error: None,
                display_name: item.display_name,
                count: None,
            }
        }
        Err(e) => FileResult {
            file: None,
            error: Some(
                serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unreadable".into()),
            ),
            display_name: item.display_name,
            count: None,
        },
    }
}

fn stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_owned(),
        _ => name.to_owned(),
    }
}

fn problems(p: Vec<converter_router::Problem>) -> String {
    p.iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A sniffer `Format` from its wire name.
pub fn parse_format(s: &str) -> Option<Format> {
    Format::parse(s)
}
