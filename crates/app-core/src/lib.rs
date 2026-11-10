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

mod files;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use converter_jobstore::{
    DataRoot, InputSource, JobError, JobEvent, JobId, JobSnapshot, JobStore, SaveCheck,
    SaveDestination, SaveOptions, Snapshot, StoreConfig, SyncRoots,
};
use converter_protocol::{Codec, SecretString, Secrets};
use converter_router::{
    build_registry, targets, Availability, Catalog, Format as RouterFormat, Platform, Registry,
    Targets,
};
use converter_sandbox::{probe, SandboxConfig, SandboxLauncher};
use converter_scheduler::{pool_for, JobSpec, Runner, RunnerConfig, SystemMemory};
use converter_sniffer::{FileDescriptor, Format};
use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use files::{FileEntry, FileWorker};
pub use files::{InspectInfo, PageText, Preview};

/// Most files in one selection (blueprint section 6.2 default).
pub const MAX_FILES: usize = 200;
/// Preview resolutions the UI may ask for.
pub const PREVIEW_DPI: &[u32] = &[48, 72, 96, 144];

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
}

type Sink = Arc<dyn Fn(&JobEvent) + Send + Sync>;

pub struct App {
    store: Arc<JobStore>,
    runner: Arc<Runner>,
    launcher: Arc<SandboxLauncher>,
    codec: Arc<Codec>,
    registry: Registry,
    files: Mutex<BTreeMap<String, FileEntry>>,
    /// File-worker jobs: never shown to the renderer.
    hidden: Arc<Mutex<HashSet<JobId>>>,
    /// Workers for previewing finished jobs' outputs, by job.
    output_workers: Mutex<BTreeMap<JobId, FileWorker>>,
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

        let catalog = Catalog::load(&config.resources).map_err(problems)?;
        let registry = build_registry(&config.adapters_dir, &catalog, config.include_dev_adapters)
            .map_err(problems)?;
        let mut sandbox_config =
            SandboxConfig::discover(&config.python, &config.adapters_dir, &config.runner)?;
        sandbox_config.runner_args = config.runner_args.clone();
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
            files: Mutex::default(),
            hidden,
            output_workers: Mutex::default(),
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
        let report = probe::self_test(&self.launcher, &scratch, home).unwrap_or_else(|_| {
            probe::ProbeReport {
                attempts: Vec::new(),
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
    pub fn add_paths(&self, paths: Vec<PathBuf>) -> CommandResult<Vec<FileResult>> {
        let mut files = lock(&self.files);
        if files.len() + paths.len() > MAX_FILES {
            return Err(CommandError::new("E_LIMIT_EXCEEDED", "tooManyFiles"));
        }
        let mut out = Vec::new();
        for path in paths {
            let display_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match converter_sniffer::sniff_path(&path) {
                Ok(descriptor) => {
                    out.push(FileResult {
                        file: Some(descriptor.clone()),
                        error: None,
                        display_name,
                    });
                    files.insert(descriptor.file_id.clone(), FileEntry::new(path, descriptor));
                }
                Err(e) => out.push(FileResult {
                    file: None,
                    error: Some(
                        serde_json::to_value(e)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unreadable".into()),
                    ),
                    display_name,
                }),
            }
        }
        Ok(out)
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
            d.actionable = format == Format::Pdf && !d.cloud_placeholder;
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

    /// Page count, encryption and warnings, from the file's sandboxed worker.
    pub fn inspect_file(&self, file_id: &str) -> CommandResult<InspectInfo> {
        self.require_sandbox()?;
        self.with_file_worker(file_id, |w, secrets| w.inspect(secrets))
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
            w.page_text("in/0.pdf", secrets, page)
        })
    }

    fn with_file_worker<T>(
        &self,
        file_id: &str,
        f: impl FnOnce(&mut FileWorker, Option<&Secrets>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let mut files = lock(&self.files);
        let entry = files
            .get_mut(file_id)
            .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
        if !entry.descriptor.actionable {
            return Err(CommandError::new("E_INTERNAL", "notActionable"));
        }
        if entry.worker.is_none() {
            let (job, _) = self.store.create_job().map_err(CommandError::from_job)?;
            lock(&self.hidden).insert(job.clone());
            self.store
                .stage_inputs(
                    &job,
                    &[InputSource {
                        path: entry.path.clone(),
                        extension: "pdf".into(),
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
                job,
                dir,
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
        if out.extension != "pdf" {
            return Err(CommandError::new("E_INTERNAL", "notPreviewable"));
        }
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
                job.clone(),
                dir,
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
        let availability = Availability::everything(Platform::current());
        Ok(targets(&self.registry, &availability, &formats))
    }

    /// Every adapter's manifest and parameter schema, for generated forms.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    // ---------------------------------------------------------------- jobs

    /// Validates params, stages inputs and starts the job (spec 11.1).
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
        let manifest = &entry.manifest;
        let (inputs, outputs) = match (&manifest.inputs, &manifest.outputs) {
            (Some(i), Some(o)) => (i, o),
            _ => return Err(CommandError::new("E_INTERNAL", "notAnOperation")),
        };

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
        if count < inputs.min || count > inputs.max {
            return Err(CommandError::new("E_INTERNAL", "wrongFileCount"));
        }

        let (sources, total_bytes, password, titles) = {
            let files = lock(&self.files);
            let mut sources = Vec::new();
            let mut total = 0;
            let mut password: Option<SecretString> = None;
            let mut titles = Vec::new();
            for id in &ordered {
                let entry = files
                    .get(id)
                    .ok_or_else(|| CommandError::new("E_UNKNOWN_FILE", "unknownFile"))?;
                let format = RouterFormat::parse(entry.descriptor.format.as_str());
                if !entry.descriptor.actionable
                    || !format.is_some_and(|f| inputs.formats.contains(&f))
                {
                    return Err(CommandError::new("E_INTERNAL", "unsupportedInput"));
                }
                total += entry.descriptor.size_bytes;
                if password.is_none() {
                    password = entry
                        .password
                        .as_ref()
                        .map(|p| SecretString::new(p.to_string()));
                }
                titles.push(stem(&entry.descriptor.display_name));
                sources.push(InputSource {
                    path: entry.path.clone(),
                    extension: entry.descriptor.format.as_str().to_owned(),
                });
            }
            (sources, total, password, titles)
        };

        // The core resolves file IDs; the worker only ever sees input order.
        if let Some(obj) = params.as_object_mut() {
            obj.remove("order");
            if operation_id == "pdf-merge" && !obj.contains_key("fileTitles") {
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

        let (job, _) = self.store.create_job().map_err(CommandError::from_job)?;
        if let Err(e) = self
            .store
            .stage_inputs(&job, &sources, manifest.limits.disk_factor)
        {
            let failure = converter_jobstore::JobFailure {
                code: e.code().unwrap_or("E_INTERNAL").into(),
                retryable: false,
            };
            let _ = self.store.fail(&job, failure);
            return Err(CommandError::from_job(e));
        }
        let output_format = parse_format(outputs.format.as_str())
            .ok_or_else(|| CommandError::new("E_INSTALL_CORRUPT", "unknownOutputFormat"))?;
        let _ = total_bytes;
        self.runner.start(JobSpec {
            job: job.clone(),
            adapter_id: manifest.id.clone(),
            pool: pool_for(&manifest.id, worker_class(manifest)),
            output_format,
            timeout_base_s: manifest.limits.timeout_base_s,
            timeout_s_per_mb: manifest.limits.timeout_s_per_mb,
            no_progress_s: manifest.limits.no_progress_s,
            params,
            secrets: has_secrets.then_some(secrets),
        });
        Ok(job)
    }

    pub fn cancel_job(&self, job: &JobId) -> CommandResult<bool> {
        Ok(self.runner.cancel(job))
    }

    pub fn discard_job(&self, job: &JobId) -> CommandResult<()> {
        self.close_output_worker(job);
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
        self.store
            .save_outputs(
                job,
                dest,
                SaveOptions {
                    confirmed_input_overwrite,
                },
            )
            .map(|r| r.saved.len())
            .map_err(CommandError::from_job)
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

fn worker_class(m: &converter_router::Manifest) -> &'static str {
    use converter_router::manifest::WorkerClass;
    match m.worker_class {
        WorkerClass::Pdf => "pdf",
        WorkerClass::Image => "image",
        WorkerClass::Render => "render",
        WorkerClass::Office => "office",
        WorkerClass::Ocr => "ocr",
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

/// A sniffer `Format` from its wire name (the sniffer type has no parser).
pub fn parse_format(s: &str) -> Option<Format> {
    use Format::*;
    [
        Pdf, Docx, Pptx, Xlsx, Md, Html, Txt, Png, Jpg, Webp, Svg, Heic,
    ]
    .into_iter()
    .find(|f| f.as_str() == s)
}
