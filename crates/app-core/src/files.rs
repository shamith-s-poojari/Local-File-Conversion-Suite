//! Opened files and their long-lived sandboxed workers.
//!
//! A file worker runs the adapter that can inspect and preview the file's
//! format (`pdf-redact` for PDFs, `image-convert` for images, and so on;
//! see `preview_adapter`) in the file's own staged job directory.
//! It stays up while the file is open, so preview pages come back quickly
//! (spike S3 targets: page 1 in 3 s, then about 300 ms a page).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use converter_jobstore::JobId;
use converter_protocol::{Codec, SecretString, Secrets};
use converter_sandbox::SandboxLauncher;
use converter_scheduler::{CancelToken, LaunchSpec, RequestKind, Timing, Watchdog, Worker};
use converter_sniffer::FileDescriptor;
use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::{CommandError, CommandResult};

pub(crate) struct FileEntry {
    pub path: PathBuf,
    pub descriptor: FileDescriptor,
    pub password: Option<Zeroizing<String>>,
    pub worker: Option<FileWorker>,
    /// For closing the least recently used worker first.
    pub last_used: std::time::Instant,
}

impl FileEntry {
    pub fn new(path: PathBuf, descriptor: FileDescriptor) -> Self {
        FileEntry {
            path,
            descriptor,
            password: None,
            worker: None,
            last_used: std::time::Instant::now(),
        }
    }

    pub fn secrets(&self) -> Option<Secrets> {
        self.password.as_ref().map(|p| Secrets {
            password: Some(SecretString::new(p.to_string())),
            ..Secrets::default()
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectInfo {
    pub pages: u64,
    pub encryption: String,
    pub signed: Option<bool>,
    pub warnings: Vec<String>,
    /// Image, audio or video facts (ADR 0003).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<Value>,
}

impl InspectInfo {
    /// For formats no worker inspects (documents and data).
    pub fn unknown() -> Self {
        InspectInfo {
            pages: 0,
            encryption: "none".into(),
            signed: None,
            warnings: Vec::new(),
            media: None,
        }
    }
}

/// Where the renderer loads a page image from: `preview://localhost/<path>`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub path: String,
    pub width_px: u64,
    pub height_px: u64,
}

/// Word boxes in PDF points, origin top-left of the displayed page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageText {
    pub width: f64,
    pub height: f64,
    pub words: Value,
}

pub(crate) struct FileWorker {
    pub job: JobId,
    /// The staged input, e.g. `in/0.png`.
    pub input: String,
    worker: Worker,
    size_bytes: u64,
    /// Preview token per input and DPI; each maps to its own folder of
    /// `<page>.png`, so a job's outputs never overwrite each other's previews.
    pub tokens: BTreeMap<(String, u32), String>,
    dir: PathBuf,
}

impl FileWorker {
    pub fn start(
        launcher: &SandboxLauncher,
        codec: Arc<Codec>,
        adapter: &str,
        job: JobId,
        dir: PathBuf,
        input: String,
        size_bytes: u64,
    ) -> CommandResult<Self> {
        let spec = LaunchSpec {
            adapter_id: adapter.into(),
            job_id: job.clone(),
            job_dir: dir.clone(),
            memory_limit: None,
        };
        let worker = Worker::spawn(
            launcher,
            &spec,
            codec,
            Timing {
                hello: std::time::Duration::from_secs(30),
                ..Timing::default()
            },
        )
        .map_err(|f| CommandError::new(&f.code, "workerStart"))?;
        Ok(FileWorker {
            job,
            input,
            worker,
            size_bytes,
            tokens: BTreeMap::new(),
            dir,
        })
    }

    fn request(
        &mut self,
        kind: RequestKind,
        body: Value,
        secrets: Option<&Secrets>,
    ) -> CommandResult<Value> {
        let watchdog = Watchdog::from_limits(60, 1.0, 60, self.size_bytes);
        self.worker
            .request(
                kind,
                body,
                secrets,
                watchdog,
                &CancelToken::new(),
                &mut |_| {},
            )
            .map_err(|f| CommandError::new(&f.code, "worker"))
    }

    pub fn inspect(&mut self, secrets: Option<&Secrets>) -> CommandResult<InspectInfo> {
        let r = self.request(
            RequestKind::Inspect,
            json!({ "input": self.input }),
            secrets,
        )?;
        Ok(InspectInfo {
            pages: r["pages"].as_u64().unwrap_or(0),
            encryption: r["encryption"].as_str().unwrap_or("none").to_owned(),
            signed: r["signed"].as_bool(),
            warnings: r["warnings"]
                .as_array()
                .map(|w| {
                    w.iter()
                        .filter_map(|x| x["code"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            media: r.get("media").filter(|m| m.is_object()).cloned(),
        })
    }

    pub fn render_input(
        &mut self,
        secrets: Option<&Secrets>,
        page: u32,
        dpi: u32,
        register: &dyn Fn(PathBuf) -> String,
    ) -> CommandResult<Preview> {
        let input = self.input.clone();
        self.render_with(&input, secrets, page, dpi, register)
    }

    pub fn render(
        &mut self,
        input: &str,
        page: u32,
        dpi: u32,
        register: &dyn Fn(PathBuf) -> String,
    ) -> CommandResult<Preview> {
        self.render_with(input, None, page, dpi, register)
    }

    fn render_with(
        &mut self,
        input: &str,
        secrets: Option<&Secrets>,
        page: u32,
        dpi: u32,
        register: &dyn Fn(PathBuf) -> String,
    ) -> CommandResult<Preview> {
        let tag: String = input
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let sub = format!("preview_{tag}_{dpi}");
        let folder = self.dir.join("out").join(&sub);
        std::fs::create_dir_all(&folder)
            .map_err(|_| CommandError::new("E_RESOURCE_DISK", "previewFolder"))?;
        let r = self.request(
            RequestKind::Render,
            json!({ "input": input, "page": page, "dpi": dpi, "out": format!("out/{sub}/{page}.png") }),
            secrets,
        )?;
        let token = self
            .tokens
            .entry((input.to_owned(), dpi))
            .or_insert_with(|| register(folder))
            .clone();
        Ok(Preview {
            path: format!("{token}/{page}.png"),
            width_px: r["widthPx"].as_u64().unwrap_or(0),
            height_px: r["heightPx"].as_u64().unwrap_or(0),
        })
    }

    pub fn page_text(
        &mut self,
        input: &str,
        secrets: Option<&Secrets>,
        page: u32,
    ) -> CommandResult<PageText> {
        let r = self.request(
            RequestKind::PageText,
            json!({ "input": input, "page": page }),
            secrets,
        )?;
        Ok(PageText {
            width: r["width"].as_f64().unwrap_or(0.0),
            height: r["height"].as_f64().unwrap_or(0.0),
            words: r["words"].clone(),
        })
    }

    pub fn shutdown(self) {
        self.worker.shutdown();
    }
}
