//! Folder intake, pasting, skipping same-format files and saving with the
//! dropped folder's layout, through the real app.
//!
//! A plain `main` (`harness = false`): started with `--sandbox-run` it is
//! the worker runner. Needs the workspace virtualenv; skips without it. The
//! parts that run a conversion also need a passing sandbox self-test.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use converter_app_core::{App, AppConfig, FileResult, MAX_FILES};
use converter_jobstore::{JobId, JobState};
use serde_json::json;

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn python() -> PathBuf {
    if cfg!(windows) {
        repo().join(".venv/Scripts/python.exe")
    } else {
        repo().join(".venv/bin/python")
    }
}

fn open(data: &Path) -> App {
    let config = AppConfig {
        data_root: Some(data.to_path_buf()),
        resources: repo(),
        adapters_dir: repo().join("adapters"),
        python: python(),
        runner: std::env::current_exe().unwrap(),
        runner_args: vec!["--sandbox-run".into()],
        user_memory_limit: None,
        include_dev_adapters: false,
    };
    App::open(
        &config,
        Arc::new(|_| {}),
        Arc::new(|_| "t".into()),
        Arc::new(|_| {}),
    )
    .expect("the app opens")
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn folders(added: &[FileResult]) -> Vec<(String, Option<String>)> {
    added
        .iter()
        .filter_map(|r| r.file.as_ref())
        .map(|f| (f.display_name.clone(), f.folder.clone()))
        .collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sandbox-run") {
        return converter_sandbox::runner::main_with(args[2..].to_vec());
    }
    if !python().is_file() {
        println!("folders: skipped (needs `uv sync`)");
        return ExitCode::SUCCESS;
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = open(&tmp.path().join("data"));
    intake(&app, tmp.path());
    truncation(&app, tmp.path());
    paste(&app, tmp.path());
    conversion(&app, tmp.path());
    println!("folders: ok");
    ExitCode::SUCCESS
}

fn intake(app: &App, tmp: &Path) {
    let root = tmp.join("photos");
    write(&root.join("b.png"), PNG);
    write(&root.join("a.png"), PNG);
    write(&root.join("2024/trip/c.png"), PNG);
    write(&root.join(".hidden.png"), PNG);
    write(&root.join(".DS_Store"), b"junk");
    write(&root.join("Thumbs.db"), b"junk");
    #[cfg(unix)]
    {
        let outside = tmp.join("outside");
        write(&outside.join("secret.png"), PNG);
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("a.png"), root.join("alias.png")).unwrap();
    }
    let single = tmp.join("single.png");
    write(&single, PNG);

    let added = app.add_paths(vec![root.clone(), single]).unwrap();
    assert_eq!(
        folders(&added),
        [
            ("c.png".into(), Some("photos/2024/trip".into())),
            ("a.png".into(), Some("photos".into())),
            ("b.png".into(), Some("photos".into())),
            ("single.png".into(), None),
        ]
    );
    assert!(added.iter().all(|r| r.error.is_none()));
    // The renderer sees the folder, never a path.
    let json = serde_json::to_string(&added).unwrap();
    assert!(!json.contains(tmp.to_str().unwrap()));

    let empty = tmp.join("empty");
    std::fs::create_dir(&empty).unwrap();
    let r = app.add_paths(vec![empty]).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].error.as_deref(), Some("emptyFolder"));
    assert_eq!(r[0].display_name, "empty");

    for r in added {
        app.remove_file(&r.file.unwrap().file_id).unwrap();
    }
}

fn truncation(app: &App, tmp: &Path) {
    let root = tmp.join("many");
    for i in 0..MAX_FILES + 5 {
        write(&root.join(format!("d{}/f{i:03}.png", i % 3)), PNG);
    }
    let r = app.add_paths(vec![root]).unwrap();
    assert_eq!(r.iter().filter(|r| r.file.is_some()).count(), MAX_FILES);
    let last = r.last().unwrap();
    assert_eq!(last.error.as_deref(), Some("folderTruncated"));
    assert_eq!(last.count, Some(5));
    assert_eq!(last.display_name, "many");
    // The selection is full: more folders add nothing but the notice.
    let extra = tmp.join("extra");
    write(&extra.join("x.png"), PNG);
    let r2 = app.add_paths(vec![extra]).unwrap();
    assert_eq!(r2.len(), 1);
    assert_eq!(r2[0].count, Some(1));
    // Plain files over the limit still fail as a whole.
    let err = app
        .add_paths(vec![tmp.join("many/d0/f000.png")])
        .unwrap_err();
    assert_eq!(err.code, "E_LIMIT_EXCEEDED");
    for r in r.into_iter().filter_map(|r| r.file) {
        app.remove_file(&r.file_id).unwrap();
    }
}

fn pasted_files(data: &Path) -> usize {
    std::fs::read_dir(data.join("pasted"))
        .map(|d| d.count())
        .unwrap_or(0)
}

fn paste(app: &App, tmp: &Path) {
    let data = tmp.join("data");
    let r = app.add_pasted("image", PNG.to_vec()).unwrap();
    let f = r[0].file.as_ref().expect("accepted");
    assert_eq!(f.display_name, "pasted-image.png");
    assert_eq!(f.format.as_str(), "png");
    assert_eq!(pasted_files(&data), 1);
    let t = app.add_pasted("table", b"a,b\n1,2\n".to_vec()).unwrap();
    assert_eq!(t[0].file.as_ref().unwrap().format.as_str(), "csv");
    let m = app
        .add_pasted("text", b"# Title\n\nbody\n".to_vec())
        .unwrap();
    assert_eq!(m[0].display_name, "pasted-text.md");
    assert_eq!(pasted_files(&data), 3);

    assert_eq!(
        app.add_pasted("image", b"not an image".to_vec())
            .unwrap_err()
            .reason
            .as_deref(),
        Some("notAnImage")
    );
    assert!(app.add_pasted("video", vec![1]).is_err());
    assert_eq!(pasted_files(&data), 3, "refusals leave nothing behind");

    // Removing from the selection deletes the file.
    app.remove_file(&f.file_id).unwrap();
    assert_eq!(pasted_files(&data), 2);

    // A new start clears what an earlier run left.
    let other = tmp.join("other-data");
    {
        let first = open(&other);
        first.add_pasted("text", b"left behind".to_vec()).unwrap();
        assert_eq!(pasted_files(&other), 1);
    }
    let again = open(&other);
    assert_eq!(pasted_files(&other), 0);
    again.add_pasted("text", b"x".to_vec()).unwrap();
    again.shutdown();
    assert_eq!(pasted_files(&other), 0, "exit clears the folder");

    for r in [t, m] {
        app.remove_file(&r[0].file.as_ref().unwrap().file_id)
            .unwrap();
    }
    assert_eq!(pasted_files(&data), 0);
}

fn wait_terminal(app: &App, job: &JobId) -> converter_jobstore::JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let j = app
            .get_snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == job)
            .expect("job listed");
        if j.state.is_terminal() {
            return j;
        }
        assert!(
            Instant::now() < deadline,
            "job didn't finish: {:?}",
            j.state
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn conversion(app: &App, tmp: &Path) {
    let home = PathBuf::from(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .unwrap(),
    );
    let report = app.self_test(&home);
    if !report.passed || !cfg!(any(target_os = "macos", windows)) {
        println!("folders: conversion part skipped (no sandbox here)");
        return;
    }
    // A real picture, which the 1x1 constant above is not.
    let seed = tmp.join("seed.png");
    let made = std::process::Command::new(python())
        .args([
            "-I",
            "-c",
            &format!(
                "from PIL import Image\nImage.new('RGB', (8, 8), 'red').save({:?})",
                seed.to_str().unwrap()
            ),
        ])
        .status()
        .unwrap();
    assert!(made.success(), "making a test PNG");
    let png = std::fs::read(&seed).unwrap();
    let root = tmp.join("album");
    write(&root.join("one.png"), &png);
    write(&root.join("sub/two.png"), &png);
    write(&root.join("sub/deeper/three.jpg"), &png); // sniffed by content: a PNG
    let added = app.add_paths(vec![root]).unwrap();
    let ids: Vec<String> = added
        .iter()
        .map(|r| r.file.as_ref().unwrap().file_id.clone())
        .collect();
    assert_eq!(ids.len(), 3);

    // Everything is already a PNG: nothing to do.
    let err = app
        .create_conversion(&ids, "png", json!({"skipSame": true}))
        .unwrap_err();
    assert_eq!(err.reason.as_deref(), Some("allSkipped"));

    // One JPEG among the PNGs: only it is converted... first make one.
    let jpg_job = app.create_conversion(&ids[..1], "jpg", json!({})).unwrap();
    let done = wait_terminal(app, &jpg_job);
    assert_eq!(
        done.state,
        JobState::Succeeded,
        "{:?} {:?}",
        done.error,
        done.warnings
    );
    assert!(app.can_keep_layout(&jpg_job));
    app.discard_job(&jpg_job).unwrap();

    let (job, skipped) = app
        .create_conversion_counted(&ids, "webp", json!({"skipSame": true}))
        .unwrap();
    assert_eq!(skipped, 0);
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(done.outputs.len(), 3);
    assert!(done.outputs.iter().all(|o| o.source.is_some()));
    assert!(app.can_keep_layout(&job));

    let dest = tmp.join("saved");
    std::fs::create_dir(&dest).unwrap();
    let dir = app.folder_destination(&job, dest.clone(), true);
    app.check_save(&job, &dir).unwrap();
    let n = app.save_outputs(&job, &dir, false).unwrap();
    assert_eq!(n, 3);
    assert!(dest.join("album/one.webp").is_file());
    assert!(dest.join("album/sub/two.webp").is_file());
    assert!(dest.join("album/sub/deeper/three.webp").is_file());
    assert!(!app.can_keep_layout(&job), "the job is gone after saving");

    // Without the option the files land flat.
    let (job, _) = app
        .create_conversion_counted(&ids, "webp", json!({}))
        .unwrap();
    assert_eq!(wait_terminal(app, &job).state, JobState::Succeeded);
    let flat = tmp.join("flat");
    std::fs::create_dir(&flat).unwrap();
    let dir = app.folder_destination(&job, flat.clone(), false);
    app.save_outputs(&job, &dir, false).unwrap();
    assert!(flat.join("one.webp").is_file());
    assert!(!flat.join("album").exists());
    app.shutdown();
}
