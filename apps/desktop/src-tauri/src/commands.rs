//! Tauri commands (spec section 11.1). Thin wrappers over `converter_app_core`:
//! argument checks, native dialogs, and moving blocking work off the main
//! thread. The renderer passes `fileId`s and `jobId`s, never paths.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use converter_app_core::{App, CommandError, FileResult, InspectInfo, PageText, Preview};
use converter_jobstore::{JobId, SaveDestination, Snapshot};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;

/// The app, once opened. `Err` holds why it couldn't start, shown in the UI.
pub struct Core(pub Result<Arc<App>, String>);

/// Save destinations waiting for the user to confirm overwriting an input.
#[derive(Default)]
pub struct PendingSaves(pub Mutex<HashMap<String, SaveDestination>>);

type Reply<T> = Result<T, CommandError>;

fn app(core: &State<'_, Core>) -> Reply<Arc<App>> {
    core.0
        .clone()
        .map_err(|_| CommandError::new("E_INSTALL_CORRUPT", "appNotStarted"))
}

fn job_id(s: &str) -> Reply<JobId> {
    JobId::parse(s).ok_or_else(|| CommandError::new("E_UNKNOWN_JOB", "badJobId"))
}

/// Runs blocking work off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Reply<T> + Send + 'static) -> Reply<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| Err(CommandError::new("E_INTERNAL", "commandPanicked")))
}

#[tauri::command]
pub async fn pick_files(handle: AppHandle, core: State<'_, Core>) -> Reply<Vec<FileResult>> {
    let app = app(&core)?;
    blocking(move || {
        let Some(picked) = handle
            .dialog()
            .file()
            .set_title("Choose files")
            .blocking_pick_files()
        else {
            return Ok(Vec::new());
        };
        let paths: Vec<PathBuf> = picked
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .collect();
        app.add_paths(paths)
    })
    .await
}

/// Opens the native folder dialog and adds the folder's files (the folder's
/// layout is kept for saving). Like `pick_files`, the path stays in Rust.
#[tauri::command]
pub async fn pick_folder(handle: AppHandle, core: State<'_, Core>) -> Reply<Vec<FileResult>> {
    let app = app(&core)?;
    blocking(move || {
        let Some(dir) = handle
            .dialog()
            .file()
            .set_title("Choose a folder")
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
        else {
            return Ok(Vec::new());
        };
        app.add_paths(vec![dir])
    })
    .await
}

/// Adds something pasted in the window: `kind` is `image`, `table` or
/// `text`. The renderer sends the bytes; Rust checks them and writes the file.
#[tauri::command]
pub async fn add_pasted(
    core: State<'_, Core>,
    request: tauri::ipc::Request<'_>,
) -> Reply<Vec<FileResult>> {
    // Raw body: the pasted bytes, up to 50 MB, without a JSON number array.
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(CommandError::new("E_INTERNAL", "emptyPaste"));
    };
    let kind = request
        .headers()
        .get("x-paste-kind")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = bytes.clone();
    let app = app(&core)?;
    blocking(move || app.add_pasted(&kind, bytes)).await
}

#[tauri::command]
pub async fn remove_file(core: State<'_, Core>, file_id: String) -> Reply<()> {
    let app = app(&core)?;
    blocking(move || app.remove_file(&file_id)).await
}

#[tauri::command]
pub async fn resolve_file(
    core: State<'_, Core>,
    file_id: String,
    format: Option<String>,
    allow_download: Option<bool>,
) -> Reply<Value> {
    let app = app(&core)?;
    blocking(move || {
        let d = app.resolve_file(&file_id, format.as_deref(), allow_download.unwrap_or(false))?;
        Ok(serde_json::to_value(d).unwrap_or(Value::Null))
    })
    .await
}

#[tauri::command]
pub async fn submit_password(
    core: State<'_, Core>,
    file_id: String,
    password: String,
) -> Reply<InspectInfo> {
    let app = app(&core)?;
    blocking(move || app.submit_password(&file_id, password)).await
}

#[tauri::command]
pub async fn inspect_file(core: State<'_, Core>, file_id: String) -> Reply<InspectInfo> {
    let app = app(&core)?;
    blocking(move || app.inspect_file(&file_id)).await
}

#[tauri::command]
pub async fn find_duplicates(core: State<'_, Core>, file_ids: Vec<String>) -> Reply<usize> {
    let app = app(&core)?;
    blocking(move || Ok(app.duplicates(&file_ids))).await
}

#[tauri::command]
pub async fn get_targets(core: State<'_, Core>, file_ids: Vec<String>) -> Reply<Value> {
    let app = app(&core)?;
    blocking(move || {
        let t = app.get_targets(&file_ids)?;
        Ok(serde_json::to_value(t).unwrap_or(Value::Null))
    })
    .await
}

/// Manifests and parameter schemas, for generated forms.
#[tauri::command]
pub fn get_registry(core: State<'_, Core>) -> Reply<Value> {
    let app = app(&core)?;
    Ok(serde_json::to_value(app.registry()).unwrap_or(Value::Null))
}

#[tauri::command]
pub async fn create_job(
    core: State<'_, Core>,
    operation_id: String,
    file_ids: Vec<String>,
    params: Value,
) -> Reply<String> {
    let app = app(&core)?;
    blocking(move || {
        app.create_job(&operation_id, &file_ids, params)
            .map(|j| j.as_str().to_owned())
    })
    .await
}

/// Converts the files to `to` in one job (ADR 0003).
#[tauri::command]
pub async fn create_conversion(
    core: State<'_, Core>,
    file_ids: Vec<String>,
    to: String,
    params: Value,
) -> Reply<String> {
    let app = app(&core)?;
    blocking(move || {
        app.create_conversion(&file_ids, &to, params)
            .map(|j| j.as_str().to_owned())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionStarted {
    job_id: String,
    /// Files left out because they were already in the target format
    /// (`skipSame` in `params`).
    skipped: usize,
}

/// [`create_conversion`] that also says how many files `skipSame` left out.
#[tauri::command]
pub async fn create_conversion_skipping(
    core: State<'_, Core>,
    file_ids: Vec<String>,
    to: String,
    params: Value,
) -> Reply<ConversionStarted> {
    let app = app(&core)?;
    blocking(move || {
        app.create_conversion_counted(&file_ids, &to, params)
            .map(|(job, skipped)| ConversionStarted {
                job_id: job.as_str().to_owned(),
                skipped,
            })
    })
    .await
}

#[tauri::command]
pub async fn cancel_job(core: State<'_, Core>, job_id: String) -> Reply<bool> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    blocking(move || app.cancel_job(&job)).await
}

#[tauri::command]
pub async fn request_preview(
    core: State<'_, Core>,
    reference: String,
    page: u32,
    dpi: u32,
) -> Reply<Preview> {
    let app = app(&core)?;
    blocking(move || app.request_preview(&reference, page, dpi)).await
}

#[tauri::command]
pub async fn get_page_text(core: State<'_, Core>, reference: String, page: u32) -> Reply<PageText> {
    let app = app(&core)?;
    blocking(move || app.get_page_text(&reference, page)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    /// `saved`, `cancelled`, or `confirmOverwrite` (call `confirm_save`).
    status: &'static str,
    count: usize,
    /// The destination syncs to the cloud: show the notice (spec 10.6).
    sync_provider: Option<Value>,
}

/// Opens the native save dialog (a file for one output, a folder for
/// several) and saves. The path never reaches the renderer. With
/// `keep_layout`, outputs that came from a dropped folder are saved in the
/// same folders below the chosen one (only if `can_keep_layout`).
#[tauri::command]
pub async fn save_outputs(
    handle: AppHandle,
    core: State<'_, Core>,
    pending: State<'_, Arc<PendingSaves>>,
    job_id: String,
    keep_layout: Option<bool>,
) -> Reply<SaveResult> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    let pending = pending.inner().clone();
    blocking(move || {
        let keep_layout = keep_layout.unwrap_or(false);
        let dest = if app.output_count(&job)? == 1 {
            let name = app.suggested_name(&job)?;
            handle
                .dialog()
                .file()
                .set_file_name(name)
                .blocking_save_file()
                .and_then(|p| p.into_path().ok())
                .map(SaveDestination::File)
        } else {
            handle
                .dialog()
                .file()
                .set_title("Choose a folder for the files")
                .blocking_pick_folder()
                .and_then(|p| p.into_path().ok())
                .map(|dir| app.folder_destination(&job, dir, keep_layout))
        };
        let Some(dest) = dest else {
            return Ok(SaveResult {
                status: "cancelled",
                count: 0,
                sync_provider: None,
            });
        };
        let check = app.check_save(&job, &dest)?;
        let sync_provider = check
            .sync_provider
            .and_then(|p| serde_json::to_value(p).ok());
        if check.overwrites_input {
            pending
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(job_id, dest);
            return Ok(SaveResult {
                status: "confirmOverwrite",
                count: 0,
                sync_provider,
            });
        }
        let count = app.save_outputs(&job, &dest, false)?;
        Ok(SaveResult {
            status: "saved",
            count,
            sync_provider,
        })
    })
    .await
}

/// The user confirmed replacing one of the job's own inputs.
#[tauri::command]
pub async fn confirm_save(
    core: State<'_, Core>,
    pending: State<'_, Arc<PendingSaves>>,
    job_id: String,
) -> Reply<SaveResult> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    let dest = pending
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&job_id)
        .ok_or_else(|| CommandError::new("E_INTERNAL", "nothingPending"))?;
    blocking(move || {
        let count = app.save_outputs(&job, &dest, true)?;
        Ok(SaveResult {
            status: "saved",
            count,
            sync_provider: None,
        })
    })
    .await
}

/// Whether saving this job can keep a dropped folder's layout.
#[tauri::command]
pub fn can_keep_layout(core: State<'_, Core>, job_id: String) -> Reply<bool> {
    let app = app(&core)?;
    Ok(app.can_keep_layout(&self::job_id(&job_id)?))
}

/// Largest output copied as text.
const MAX_COPY_TEXT: u64 = 10 * 1024 * 1024;

/// What an output becomes on the clipboard, by its extension: a picture
/// (true) or text (false).
fn copy_kind(extension: &str) -> Option<bool> {
    match extension {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff" | "avif" | "heic" | "svg"
        | "ico" => Some(true),
        "txt" | "md" | "csv" | "json" | "yaml" | "html" => Some(false),
        _ => None,
    }
}

/// Copies a finished job's output to the clipboard: a picture as an image,
/// text-like formats as text. A picture is decoded by its sandboxed worker,
/// which hands back a PNG; the renderer never sees a path or the bytes.
/// Other formats fail with `notCopyable`.
#[tauri::command]
pub async fn copy_output(
    handle: AppHandle,
    core: State<'_, Core>,
    job_id: String,
    index: usize,
) -> Reply<()> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    blocking(move || {
        let extension = app
            .output_extension(&job, index)?
            .ok_or_else(|| CommandError::new("E_INTERNAL", "unknownOutput"))?;
        let image =
            copy_kind(&extension).ok_or_else(|| CommandError::new("E_INTERNAL", "notCopyable"))?;
        let clip = handle.clipboard();
        let written = if image {
            let png = app.clipboard_png(&job, index)?;
            let picture = tauri::image::Image::from_bytes(&png)
                .map_err(|_| CommandError::new("E_INTERNAL", "notCopyable"))?;
            clip.write_image(&picture)
        } else {
            let (_, bytes) = app.read_output(&job, index, MAX_COPY_TEXT)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| CommandError::new("E_INTERNAL", "notCopyable"))?;
            clip.write_text(text)
        };
        written.map_err(|_| CommandError::new("E_INTERNAL", "clipboardUnavailable"))
    })
    .await
}

#[tauri::command]
pub async fn discard_job(core: State<'_, Core>, job_id: String) -> Reply<()> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    blocking(move || app.discard_job(&job)).await
}

#[tauri::command]
pub fn get_snapshot(core: State<'_, Core>) -> Reply<Snapshot> {
    Ok(app(&core)?.get_snapshot())
}

/// Versions, self-test result and adapters. If the app couldn't start,
/// says why. Never file names or contents.
#[tauri::command]
pub fn get_diagnostics(core: State<'_, Core>) -> Value {
    match &core.0 {
        Ok(app) => serde_json::to_value(app.get_diagnostics()).unwrap_or(Value::Null),
        Err(why) => serde_json::json!({ "startupError": why, "processingEnabled": false }),
    }
}
