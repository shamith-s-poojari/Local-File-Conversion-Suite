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
mod jobs;
mod output;
mod recheck;
mod selection;
mod workers;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use converter_jobstore::{DataRoot, JobError, JobEvent, JobId, JobStore, StoreConfig, SyncRoots};
use converter_protocol::Codec;
use converter_router::{build_registry, Availability, Catalog, Platform, Registry};
use converter_sandbox::{probe, SandboxConfig, SandboxLauncher};
use converter_scheduler::{Runner, RunnerConfig, SystemMemory};
use converter_sniffer::{FileDescriptor, Format};
use serde::Serialize;
use serde_json::Value;

pub use config::find_dev_root;
pub use converter_jobstore::SaveReport;
pub use defaults::{defaults, with_defaults};
pub use engines::Engines;
use files::{FileEntry, OutputSlot};
pub use files::{InspectInfo, PageText, Preview};
pub use output::open_regular;

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

/// Why a command failed, beside its `E_*` code: the camelCase strings the
/// front ends key their texts on. `CommandError::reason` carries
/// [`Reason::as_str`]; adding a variant is how a new reason is introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    AllSkipped,
    AppNotStarted,
    BadJobId,
    BadParamsSchema,
    ClipboardUnavailable,
    CommandPanicked,
    DownloadFailed,
    EmptyPaste,
    EngineMissing,
    JobNotFinished,
    MultiHop,
    NoFiles,
    NoOutputs,
    NoRoute,
    NothingPending,
    NotACandidate,
    NotActionable,
    NotAnImage,
    NotAnObject,
    NotAnOperation,
    NotCopyable,
    NotPreviewable,
    NotText,
    OfficePassword,
    OrderMismatch,
    PasswordNotText,
    PasswordsDiffer,
    PasteFolder,
    PasteName,
    PasteTooLarge,
    PasteWrite,
    PreviewFolder,
    RevealFailed,
    SelfTestFailed,
    SelfTestPending,
    TooLargeToCopy,
    TooManyFiles,
    UnknownAdapter,
    UnknownFile,
    UnknownFormat,
    UnknownJob,
    UnknownOperation,
    UnknownOutput,
    UnknownOutputFormat,
    UnknownPasteKind,
    Unreadable,
    UnreadableOutput,
    UnsupportedDpi,
    UnsupportedInput,
    Worker,
    WorkerStart,
    WrongFileCount,
}

impl Reason {
    /// Every reason, for tests and for generating front-end text tables.
    pub const ALL: &'static [Reason] = &[
        Reason::AllSkipped,
        Reason::AppNotStarted,
        Reason::BadJobId,
        Reason::BadParamsSchema,
        Reason::ClipboardUnavailable,
        Reason::CommandPanicked,
        Reason::DownloadFailed,
        Reason::EmptyPaste,
        Reason::EngineMissing,
        Reason::JobNotFinished,
        Reason::MultiHop,
        Reason::NoFiles,
        Reason::NoOutputs,
        Reason::NoRoute,
        Reason::NothingPending,
        Reason::NotACandidate,
        Reason::NotActionable,
        Reason::NotAnImage,
        Reason::NotAnObject,
        Reason::NotAnOperation,
        Reason::NotCopyable,
        Reason::NotPreviewable,
        Reason::NotText,
        Reason::OfficePassword,
        Reason::OrderMismatch,
        Reason::PasswordNotText,
        Reason::PasswordsDiffer,
        Reason::PasteFolder,
        Reason::PasteName,
        Reason::PasteTooLarge,
        Reason::PasteWrite,
        Reason::PreviewFolder,
        Reason::RevealFailed,
        Reason::SelfTestFailed,
        Reason::SelfTestPending,
        Reason::TooLargeToCopy,
        Reason::TooManyFiles,
        Reason::UnknownAdapter,
        Reason::UnknownFile,
        Reason::UnknownFormat,
        Reason::UnknownJob,
        Reason::UnknownOperation,
        Reason::UnknownOutput,
        Reason::UnknownOutputFormat,
        Reason::UnknownPasteKind,
        Reason::Unreadable,
        Reason::UnreadableOutput,
        Reason::UnsupportedDpi,
        Reason::UnsupportedInput,
        Reason::Worker,
        Reason::WorkerStart,
        Reason::WrongFileCount,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::AllSkipped => "allSkipped",
            Reason::AppNotStarted => "appNotStarted",
            Reason::BadJobId => "badJobId",
            Reason::BadParamsSchema => "badParamsSchema",
            Reason::ClipboardUnavailable => "clipboardUnavailable",
            Reason::CommandPanicked => "commandPanicked",
            Reason::DownloadFailed => "downloadFailed",
            Reason::EmptyPaste => "emptyPaste",
            Reason::EngineMissing => "engineMissing",
            Reason::JobNotFinished => "jobNotFinished",
            Reason::MultiHop => "multiHop",
            Reason::NoFiles => "noFiles",
            Reason::NoOutputs => "noOutputs",
            Reason::NoRoute => "noRoute",
            Reason::NothingPending => "nothingPending",
            Reason::NotACandidate => "notACandidate",
            Reason::NotActionable => "notActionable",
            Reason::NotAnImage => "notAnImage",
            Reason::NotAnObject => "notAnObject",
            Reason::NotAnOperation => "notAnOperation",
            Reason::NotCopyable => "notCopyable",
            Reason::NotPreviewable => "notPreviewable",
            Reason::NotText => "notText",
            Reason::OfficePassword => "officePassword",
            Reason::OrderMismatch => "orderMismatch",
            Reason::PasswordNotText => "passwordNotText",
            Reason::PasswordsDiffer => "passwordsDiffer",
            Reason::PasteFolder => "pasteFolder",
            Reason::PasteName => "pasteName",
            Reason::PasteTooLarge => "pasteTooLarge",
            Reason::PasteWrite => "pasteWrite",
            Reason::PreviewFolder => "previewFolder",
            Reason::RevealFailed => "revealFailed",
            Reason::SelfTestFailed => "selfTestFailed",
            Reason::SelfTestPending => "selfTestPending",
            Reason::TooLargeToCopy => "tooLargeToCopy",
            Reason::TooManyFiles => "tooManyFiles",
            Reason::UnknownAdapter => "unknownAdapter",
            Reason::UnknownFile => "unknownFile",
            Reason::UnknownFormat => "unknownFormat",
            Reason::UnknownJob => "unknownJob",
            Reason::UnknownOperation => "unknownOperation",
            Reason::UnknownOutput => "unknownOutput",
            Reason::UnknownOutputFormat => "unknownOutputFormat",
            Reason::UnknownPasteKind => "unknownPasteKind",
            Reason::Unreadable => "unreadable",
            Reason::UnreadableOutput => "unreadableOutput",
            Reason::UnsupportedDpi => "unsupportedDpi",
            Reason::UnsupportedInput => "unsupportedInput",
            Reason::Worker => "worker",
            Reason::WorkerStart => "workerStart",
            Reason::WrongFileCount => "wrongFileCount",
        }
    }
}

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
    /// For callers with a reason of their own; the core uses [`CommandError::with`].
    pub fn new(code: &str, reason: &str) -> Self {
        CommandError {
            code: code.into(),
            reason: Some(reason.into()),
            issues: Vec::new(),
        }
    }

    pub fn with(code: &str, reason: Reason) -> Self {
        Self::new(code, reason.as_str())
    }

    pub(crate) fn from_job(e: JobError) -> Self {
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
    // No sync-folder field: the data root refuses to live in a sync folder
    // (see `DataRoot::open`), so it could only ever have said "none".
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
    ///
    /// Never held across worker I/O, sniffing, downloads or file copies: a
    /// slow file must not freeze the others. A worker in use is taken out of
    /// its entry (`busy`) and put back when the call ends; `files_idle` wakes
    /// callers waiting for it.
    files: Mutex<BTreeMap<String, FileEntry>>,
    files_idle: Condvar,
    /// File-worker jobs: never shown to the renderer.
    hidden: Arc<Mutex<HashSet<JobId>>>,
    /// Workers for previewing finished jobs' outputs, by job, with the same
    /// take-out-while-in-use rule as `files`.
    output_workers: Mutex<BTreeMap<JobId, OutputSlot>>,
    output_idle: Condvar,
    /// Threads of started jobs, joined by [`App::shutdown`].
    runners: Mutex<Vec<JoinHandle<()>>>,
    /// Per job, per staged input: the dropped folder it came from
    /// (`root/relative/dir`), for keeping the layout when saving.
    layouts: Mutex<BTreeMap<JobId, Vec<Option<String>>>>,
    /// Per job: the file ID of each staged input, in staged order.
    inputs: Mutex<BTreeMap<JobId, Vec<String>>>,
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
            files_idle: Condvar::new(),
            hidden,
            output_workers: Mutex::default(),
            output_idle: Condvar::new(),
            runners: Mutex::default(),
            layouts: Mutex::default(),
            inputs: Mutex::default(),
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
            Some(_) => Err(CommandError::with(
                "E_SANDBOX_UNAVAILABLE",
                Reason::SelfTestFailed,
            )),
            // No report yet: the test hasn't finished (or been started), which
            // isn't the same as a failed one.
            None => Err(CommandError::with(
                "E_SANDBOX_UNAVAILABLE",
                Reason::SelfTestPending,
            )),
        }
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
                // The output checker is internal, not a tool (ADR 0006).
                .filter(|a| a.manifest.kind != converter_router::manifest::Kind::Check)
                .map(|a| a.manifest.id.clone())
                .collect(),
            processing_enabled: sandbox.as_ref().is_some_and(|r| r.passed),
            sandbox,
            engines: self.engines.installed.iter().cloned().collect(),
            platform: std::env::consts::OS,
        }
    }

    /// How long [`App::shutdown`] waits for running jobs to stop.
    pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

    /// App exit: stop workers, then delete every job directory (spec 10.5).
    pub fn shutdown(&self) {
        self.shutdown_within(Self::SHUTDOWN_WAIT);
    }

    /// [`App::shutdown`] with its own wait. Every job is cancelled and its
    /// runner thread joined for at most `wait` before the store deletes the
    /// job directories, so no worker is still writing into one. A runner that
    /// hasn't stopped by then is left behind (its worker is killed with the
    /// process).
    pub fn shutdown_within(&self, wait: Duration) {
        let entries: Vec<FileEntry> = std::mem::take(&mut *lock(&self.files))
            .into_values()
            .collect();
        self.files_idle.notify_all();
        for job in self.store.snapshot().jobs {
            self.runner.cancel(&job.job_id);
        }
        for entry in entries {
            self.close_entry(entry);
        }
        let jobs: Vec<JobId> = lock(&self.output_workers).keys().cloned().collect();
        for job in jobs {
            self.close_output_worker(&job);
        }
        self.join_runners(wait);
        lock(&self.layouts).clear();
        lock(&self.inputs).clear();
        intake::clear_pasted(&self.pasted_dir);
        self.store.shutdown();
    }

    fn join_runners(&self, wait: Duration) {
        let deadline = Instant::now() + wait;
        let mut handles = std::mem::take(&mut *lock(&self.runners));
        while !handles.is_empty() {
            let (done, running): (Vec<_>, Vec<_>) =
                handles.into_iter().partition(|h| h.is_finished());
            for h in done {
                let _ = h.join();
            }
            handles = running;
            if handles.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_unique_camel_case_words() {
        let mut seen = HashSet::new();
        for r in Reason::ALL {
            let s = r.as_str();
            assert!(seen.insert(s), "{s} twice");
            assert!(s.chars().next().unwrap().is_ascii_lowercase(), "{s}");
            assert!(s.chars().all(|c| c.is_ascii_alphanumeric()), "{s}");
        }
    }

    #[test]
    fn a_new_reason_for_a_pending_self_test_exists() {
        let e = CommandError::with("E_SANDBOX_UNAVAILABLE", Reason::SelfTestPending);
        assert_eq!(e.reason.as_deref(), Some("selfTestPending"));
        assert_ne!(
            Reason::SelfTestPending.as_str(),
            Reason::SelfTestFailed.as_str()
        );
    }

    #[test]
    fn every_reason_in_the_source_is_in_the_enum() {
        // A reason typed as a string would bypass the enum's list; the core's
        // own code must use `CommandError::with`.
        for file in ["selection", "workers", "jobs", "output", "intake", "files"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(format!("{file}.rs"));
            let text = std::fs::read_to_string(path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap();
            assert!(!code.contains("CommandError::new("), "{file}.rs");
        }
    }
}
