//! Tauri commands (spec section 11.1). Thin wrappers over `converter_app_core`:
//! argument checks, native dialogs, and moving blocking work off the main
//! thread. The renderer passes `fileId`s and `jobId`s, never paths.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use converter_app_core::{
    App, CommandError, FileResult, InspectInfo, PageText, Preview, Reason, SaveReport,
};
use converter_jobstore::{JobId, SaveDestination, Snapshot};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;

use crate::tokens::{PendingSaves, RevealTokens};

/// The app, once opened. `Err` holds why it couldn't start, shown in the UI.
pub struct Core(pub Result<Arc<App>, String>);

type Reply<T> = Result<T, CommandError>;

fn app(core: &State<'_, Core>) -> Reply<Arc<App>> {
    core.0
        .clone()
        .map_err(|_| CommandError::with("E_INSTALL_CORRUPT", Reason::AppNotStarted))
}

fn job_id(s: &str) -> Reply<JobId> {
    JobId::parse(s).ok_or_else(|| CommandError::with("E_UNKNOWN_JOB", Reason::BadJobId))
}

/// Runs blocking work off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Reply<T> + Send + 'static) -> Reply<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| Err(CommandError::with("E_INTERNAL", Reason::CommandPanicked)))
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
        return Err(CommandError::with("E_INTERNAL", Reason::EmptyPaste));
    };
    let kind = request
        .headers()
        .get("x-paste-kind")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let app = app(&core)?;
    // The request borrows its body, so it can't move to a blocking task, and
    // copying up to 50 MB to get it there is what this avoids (audit R13).
    // The write runs here, on the async runtime and not the main thread,
    // which is bounded by the 50 MB limit; a panic still becomes an error.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.add_pasted(&kind, bytes)
    }))
    .unwrap_or_else(|_| Err(CommandError::with("E_INTERNAL", Reason::CommandPanicked)))
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
    /// `saved`, `cancelled`, or `confirmOverwrite` (call `confirm_save` with
    /// `confirm_token`, or `cancel_save`).
    status: &'static str,
    count: usize,
    /// The destination syncs to the cloud: show the notice (spec 10.6).
    sync_provider: Option<Value>,
    /// With `confirmOverwrite`: the one-time token `confirm_save` and
    /// `cancel_save` need.
    confirm_token: Option<String>,
    /// For display only: the saved file, or the folder when there are several.
    path: Option<String>,
    /// One-time token for `reveal_saved` (audit U7).
    reveal_token: Option<String>,
}

impl SaveResult {
    fn cancelled() -> Self {
        SaveResult {
            status: "cancelled",
            count: 0,
            sync_provider: None,
            confirm_token: None,
            path: None,
            reveal_token: None,
        }
    }

    fn saved(report: &SaveReport, dest: &SaveDestination, reveal: &RevealTokens) -> Self {
        let shown = shown_path(report, dest);
        SaveResult {
            status: "saved",
            count: report.saved.len(),
            sync_provider: report
                .sync_provider
                .as_ref()
                .and_then(|p| serde_json::to_value(p).ok()),
            confirm_token: None,
            path: shown.as_ref().map(|p| p.display().to_string()),
            reveal_token: shown.map(|p| reveal.issue(p)),
        }
    }
}

/// What "Show in folder" points at: the file when there is one output, else
/// the folder the user chose.
fn shown_path(report: &SaveReport, dest: &SaveDestination) -> Option<PathBuf> {
    if let [only] = report.saved.as_slice() {
        return Some(only.clone());
    }
    match dest {
        SaveDestination::File(p) => Some(p.clone()),
        SaveDestination::Folder(dir) => Some(dir.clone()),
        SaveDestination::FolderKeepingLayout(layout) => Some(layout.dir.clone()),
    }
}

/// Opens the native save dialog (a file for one output, a folder for
/// several) and saves. The path never reaches the renderer (it gets a
/// display string and tokens). With `keep_layout`, outputs that came from a
/// dropped folder are saved in the same folders below the chosen one (only
/// if `can_keep_layout`).
#[tauri::command]
pub async fn save_outputs(
    handle: AppHandle,
    core: State<'_, Core>,
    pending: State<'_, Arc<PendingSaves>>,
    reveal: State<'_, Arc<RevealTokens>>,
    job_id: String,
    keep_layout: Option<bool>,
) -> Reply<SaveResult> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    let pending = pending.inner().clone();
    let reveal = reveal.inner().clone();
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
            return Ok(SaveResult::cancelled());
        };
        let check = app.check_save(&job, &dest)?;
        if check.overwrites_input {
            let sync_provider = check
                .sync_provider
                .and_then(|p| serde_json::to_value(p).ok());
            let token = pending.hold(&job_id, dest);
            return Ok(SaveResult {
                status: "confirmOverwrite",
                count: 0,
                sync_provider,
                confirm_token: Some(token),
                path: None,
                reveal_token: None,
            });
        }
        let report = app.save_outputs(&job, &dest, false)?;
        // The save deleted the job: nothing can be waiting for it.
        pending.clear_job(&job_id);
        Ok(SaveResult::saved(&report, &dest, &reveal))
    })
    .await
}

/// The user confirmed replacing one of the job's own inputs. `token` is the
/// one `save_outputs` returned with `confirmOverwrite`.
#[tauri::command]
pub async fn confirm_save(
    core: State<'_, Core>,
    pending: State<'_, Arc<PendingSaves>>,
    reveal: State<'_, Arc<RevealTokens>>,
    job_id: String,
    token: String,
) -> Reply<SaveResult> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    let dest = pending
        .take(&job_id, &token)
        .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NothingPending))?;
    let pending = pending.inner().clone();
    let reveal = reveal.inner().clone();
    blocking(move || {
        let report = app.save_outputs(&job, &dest, true)?;
        pending.clear_job(&job_id);
        Ok(SaveResult::saved(&report, &dest, &reveal))
    })
    .await
}

/// The user said no to replacing an input: forget the held destination.
#[tauri::command]
pub fn cancel_save(
    pending: State<'_, Arc<PendingSaves>>,
    job_id: String,
    token: String,
) -> Reply<()> {
    if pending.cancel(&job_id, &token) {
        Ok(())
    } else {
        Err(CommandError::with("E_INTERNAL", Reason::NothingPending))
    }
}

/// Shows what a save wrote in Finder, Explorer or the file manager. `token`
/// is `SaveResult::reveal_token`: it works once, for ten minutes.
#[tauri::command]
pub async fn reveal_saved(reveal: State<'_, Arc<RevealTokens>>, token: String) -> Reply<()> {
    let path = reveal
        .take(&token)
        .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NothingPending))?;
    blocking(move || show_in_file_manager(&path)).await
}

/// `open -R FILE` (Finder selects it), `explorer /select,FILE`, or
/// `xdg-open FOLDER`; a folder is opened itself.
fn show_in_file_manager(path: &Path) -> Reply<()> {
    // Never let a path be read as an option by the program started below.
    if !path.is_absolute() || !path.exists() {
        return Err(CommandError::with("E_INTERNAL", Reason::RevealFailed));
    }
    let mut cmd = reveal_command(path);
    // Explorer exits with 1 even when it worked; only a failure to start counts.
    cmd.status()
        .map(|_| ())
        .map_err(|_| CommandError::with("E_INTERNAL", Reason::RevealFailed))
}

fn reveal_command(path: &Path) -> std::process::Command {
    use std::process::Command;
    let is_dir = path.is_dir();
    if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        if !is_dir {
            c.arg("-R");
        }
        c.arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        if is_dir {
            c.arg(path);
        } else {
            // One argument, `/select,` joined to the path, as Explorer wants.
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(path);
            c.arg(arg);
        }
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(if is_dir {
            path
        } else {
            path.parent().unwrap_or(path)
        });
        c
    }
}

/// The file IDs of a job's staged inputs, in staged order: what a warning's
/// `detail.inputs` indices refer to.
#[tauri::command]
pub fn job_inputs(core: State<'_, Core>, job_id: String) -> Reply<Vec<String>> {
    let app = app(&core)?;
    app.job_input_files(&self::job_id(&job_id)?)
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
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownOutput))?;
        let image = copy_kind(&extension)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NotCopyable))?;
        let clip = handle.clipboard();
        let written = if image {
            let png = app.clipboard_png(&job, index)?;
            let picture = tauri::image::Image::from_bytes(&png)
                .map_err(|_| CommandError::with("E_INTERNAL", Reason::NotCopyable))?;
            clip.write_image(&picture)
        } else {
            let (_, bytes) = app.read_output(&job, index, MAX_COPY_TEXT)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| CommandError::with("E_INTERNAL", Reason::NotCopyable))?;
            clip.write_text(text)
        };
        written.map_err(|_| CommandError::with("E_INTERNAL", Reason::ClipboardUnavailable))
    })
    .await
}

#[tauri::command]
pub async fn discard_job(
    core: State<'_, Core>,
    pending: State<'_, Arc<PendingSaves>>,
    job_id: String,
) -> Reply<()> {
    let app = app(&core)?;
    let job = self::job_id(&job_id)?;
    // A confirmation held for a job that is going away can never be used.
    pending.clear_job(&job_id);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(c: &std::process::Command) -> (String, Vec<String>) {
        (
            c.get_program().to_string_lossy().into_owned(),
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn reveal_selects_a_file_and_opens_a_folder() {
        let dir = std::env::temp_dir().join(format!("reveal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.pdf");
        std::fs::write(&file, b"x").unwrap();
        let (file_s, dir_s) = (
            file.to_string_lossy().into_owned(),
            dir.to_string_lossy().into_owned(),
        );

        let (prog, a) = args(&reveal_command(&file));
        let (dprog, da) = args(&reveal_command(&dir));
        if cfg!(target_os = "macos") {
            assert_eq!((prog.as_str(), a), ("open", vec!["-R".to_string(), file_s]));
            assert_eq!((dprog.as_str(), da), ("open", vec![dir_s]));
        } else if cfg!(windows) {
            assert_eq!(
                (prog.as_str(), a),
                ("explorer", vec![format!("/select,{file_s}")])
            );
            assert_eq!((dprog.as_str(), da), ("explorer", vec![dir_s]));
        } else {
            // A file manager can't select a file portably: it opens the folder.
            assert_eq!((prog.as_str(), a), ("xdg-open", vec![dir_s.clone()]));
            assert_eq!((dprog.as_str(), da), ("xdg-open", vec![dir_s]));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reveal_refuses_relative_and_missing_paths() {
        // A relative path could be read as an option by the program started.
        assert!(show_in_file_manager(Path::new("-R")).is_err());
        assert!(show_in_file_manager(Path::new("relative/out.pdf")).is_err());
        let missing = std::env::temp_dir().join("lfcs-reveal-missing-0123456789");
        assert!(show_in_file_manager(&missing).is_err());
    }
}
