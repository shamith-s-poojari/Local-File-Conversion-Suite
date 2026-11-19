//! The release build must keep unwinding. The runner (`runner.rs`), the worker
//! pool and the Tauri commands use `catch_unwind` to turn a panic into one
//! failed job; with `panic = "abort"` the same panic closes the whole app.

#[test]
fn release_profile_keeps_unwinding() {
    let manifest = include_str!("../../../Cargo.toml");
    let profile = manifest
        .split("[profile.release]")
        .nth(1)
        .expect("the workspace has a release profile");
    let profile = profile.split("\n[").next().unwrap_or(profile);
    let aborts = profile
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").replace(' ', ""))
        .any(|l| l == "panic=\"abort\"");
    assert!(!aborts, "release profile sets panic = \"abort\"");
}
