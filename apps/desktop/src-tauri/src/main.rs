// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod navigation;
mod preview;

use std::path::PathBuf;
use std::sync::Arc;

use converter_app_core::{App, AppConfig};
use tauri::webview::NewWindowResponse;
use tauri::{DragDropEvent, Emitter, Manager, RunEvent, WebviewWindowBuilder, WindowEvent};

use commands::{Core, PendingSaves};

/// Started as `<app> --sandbox-run …`, the executable is the worker runner
/// (see `converter_sandbox::runner`), so releases ship no extra binary.
const RUNNER_FLAG: &str = "--sandbox-run";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(RUNNER_FLAG) {
        return converter_sandbox::runner::main_with(args[2..].to_vec());
    }
    run();
    std::process::ExitCode::SUCCESS
}

/// Where the runtime, adapters and catalog live. Development uses the
/// repository and its virtualenv; packaged builds (T21) use bundle resources.
fn app_config(handle: &tauri::AppHandle) -> Result<AppConfig, String> {
    let runner = std::env::current_exe().map_err(|e| e.to_string())?;
    let (resources, python) = if cfg!(debug_assertions) {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let repo = repo.canonicalize().map_err(|e| e.to_string())?;
        let python = if cfg!(windows) {
            repo.join(".venv/Scripts/python.exe")
        } else {
            repo.join(".venv/bin/python")
        };
        (repo, python)
    } else {
        let res = handle.path().resource_dir().map_err(|e| e.to_string())?;
        let python = if cfg!(windows) {
            res.join("python/python.exe")
        } else {
            res.join("python/bin/python3")
        };
        (res, python)
    };
    if !python.is_file() {
        return Err("the worker runtime is missing (run `uv sync` in development)".into());
    }
    Ok(AppConfig {
        data_root: None,
        adapters_dir: resources.join("adapters"),
        resources,
        python,
        runner,
        runner_args: vec![RUNNER_FLAG.into()],
        user_memory_limit: None,
        include_dev_adapters: false,
    })
}

fn open_core(handle: &tauri::AppHandle, previews: Arc<preview::PreviewRegistry>) -> Core {
    let config = match app_config(handle) {
        Ok(c) => c,
        Err(e) => return Core(Err(e)),
    };
    let events = handle.clone();
    let register = previews.clone();
    let opened = App::open(
        &config,
        Arc::new(move |e| {
            let _ = events.emit("job", e);
        }),
        Arc::new(move |dir| register.register(dir)),
        Arc::new(move |token| previews.revoke(token)),
    );
    Core(opened.map(Arc::new))
}

fn run() {
    let previews = Arc::new(preview::PreviewRegistry::default());
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(previews.clone())
        .manage(Arc::new(PendingSaves::default()))
        .register_uri_scheme_protocol("preview", |ctx, request| {
            preview::handle(
                &ctx.app_handle().state::<Arc<preview::PreviewRegistry>>(),
                &request,
            )
        })
        .invoke_handler(tauri::generate_handler![
            commands::pick_files,
            commands::remove_file,
            commands::resolve_file,
            commands::submit_password,
            commands::inspect_file,
            commands::get_targets,
            commands::get_registry,
            commands::create_job,
            commands::cancel_job,
            commands::request_preview,
            commands::get_page_text,
            commands::save_outputs,
            commands::confirm_save,
            commands::discard_job,
            commands::get_snapshot,
            commands::get_diagnostics,
        ])
        .setup(move |app| {
            let core = open_core(app.handle(), previews.clone());
            let ready = core.0.clone();
            app.manage(core);

            // The window is declared in tauri.conf.json with `create: false`
            // so the navigation guards below are attached before anything loads.
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned()
                .ok_or("tauri.conf.json has no window labelled \"main\"")?;
            let allow_dev_server = cfg!(debug_assertions);
            let window = WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_navigation(move |url| navigation::is_allowed(url, allow_dev_server))
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .build()?;

            if let Ok(core) = ready {
                // Drops are handled natively: the renderer never gets the files.
                let drops = core.clone();
                let handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                        let core = drops.clone();
                        let handle = handle.clone();
                        let paths = paths.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            let added = core.add_paths(paths);
                            let _ = handle.emit("files-added", added);
                        });
                    }
                });

                // The sandbox self-test gates processing (spec 9.3).
                let handle = app.handle().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let home = handle
                        .path()
                        .home_dir()
                        .unwrap_or_else(|_| PathBuf::from("/"));
                    let report = core.self_test(&home);
                    let _ = handle.emit("selftest", report);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start the application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Kill workers and delete every job directory (spec 10.5).
            if let Ok(core) = &handle.state::<Core>().0 {
                core.shutdown();
            }
        }
    });
}
