// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod navigation;
mod preview;

use tauri::webview::NewWindowResponse;
use tauri::{Manager, WebviewWindowBuilder};

fn main() {
    tauri::Builder::default()
        .manage(preview::PreviewRegistry::default())
        .register_uri_scheme_protocol("preview", |ctx, request| {
            preview::handle(
                &ctx.app_handle().state::<preview::PreviewRegistry>(),
                &request,
            )
        })
        .setup(|app| {
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
            WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_navigation(move |url| navigation::is_allowed(url, allow_dev_server))
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the application");
}
