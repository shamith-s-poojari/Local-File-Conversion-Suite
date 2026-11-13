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
/// several) and saves. The path never reaches the renderer.
#[tauri::command]
pub async fn save_outputs(
    handle: AppHandle,
    core: State<'_, Core>,
    pending: State<'_, Arc<PendingSaves>>,
    job_id: String,
) -> Reply<SaveResult> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    let pending = pending.inner().clone();
    blocking(move || {
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
                .map(SaveDestination::Folder)
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
