//! Every app command is declared here, so Tauri generates an `allow-<name>`
//! permission for each and the single capability file can list them by name
//! (spec 11.4). A command missing from this list can't be called.

const COMMANDS: &[&str] = &[
    "pick_files",
    "pick_folder",
    "add_pasted",
    "remove_file",
    "resolve_file",
    "submit_password",
    "inspect_file",
    "find_duplicates",
    "get_targets",
    "get_registry",
    "create_job",
    "create_conversion",
    "create_conversion_skipping",
    "cancel_job",
    "request_preview",
    "get_page_text",
    "save_outputs",
    "confirm_save",
    "can_keep_layout",
    "copy_output",
    "discard_job",
    "get_snapshot",
    "get_diagnostics",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build failed");
}
