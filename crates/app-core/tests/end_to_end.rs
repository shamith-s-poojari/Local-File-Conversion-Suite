//! The whole app path on this machine: self-test, files, targets, inspect,
//! previews, page text, jobs through the sandbox, saving and cleanup.
//!
//! A plain `main` (`harness = false`): started with `--sandbox-run` it is
//! the worker runner, exactly as the desktop app is. Needs the workspace
//! virtualenv; skips without it, and on platforms with no sandbox yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use converter_app_core::{App, AppConfig};
use converter_jobstore::{JobId, JobState, SaveDestination};
use serde_json::json;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sandbox-run") {
        return converter_sandbox::runner::main_with(args[2..].to_vec());
    }
    let python = repo().join(".venv/bin/python");
    if !python.is_file() || !cfg!(target_os = "macos") {
        println!("end_to_end: skipped (needs `uv sync`, and macOS until Linux CI proves bwrap)");
        return ExitCode::SUCCESS;
    }
    scenario(&python);
    println!("end_to_end: ok");
    ExitCode::SUCCESS
}

fn make_pdf(python: &Path, path: &Path, pages: u32) {
    let code = format!(
        "import pikepdf\n\
         pdf = pikepdf.new()\n\
         font = pdf.make_indirect(pikepdf.Dictionary(Type=pikepdf.Name.Font, Subtype=pikepdf.Name.Type1, BaseFont=pikepdf.Name.Helvetica))\n\
         for n in range(1, {pages} + 1):\n\
         \x20   c = pdf.make_stream(('BT /F1 24 Tf 72 700 Td (Page %d) Tj ET' % n).encode())\n\
         \x20   pdf.pages.append(pikepdf.Page(pdf.make_indirect(pikepdf.Dictionary(Type=pikepdf.Name.Page, MediaBox=[0,0,612,792], Resources=pikepdf.Dictionary(Font=pikepdf.Dictionary(F1=font)), Contents=c))))\n\
         pdf.save({:?})\n",
        path.to_str().unwrap()
    );
    let ok = Command::new(python)
        .args(["-I", "-c", &code])
        .status()
        .unwrap()
        .success();
    assert!(ok, "making a test PDF");
}

fn wait_terminal(app: &App, job: &JobId) -> converter_jobstore::JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let snap = app.get_snapshot();
        let j = snap
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

fn scenario(python: &Path) {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Alpha report.pdf");
    let b = tmp.path().join("beta.pdf");
    make_pdf(python, &a, 2);
    make_pdf(python, &b, 3);

    let events = Arc::new(Mutex::new(Vec::new()));
    let previews: Arc<Mutex<HashMap<String, PathBuf>>> = Arc::default();
    let config = AppConfig {
        data_root: Some(tmp.path().join("data")),
        resources: repo(),
        adapters_dir: repo().join("adapters"),
        python: python.to_path_buf(),
        runner: std::env::current_exe().unwrap(),
        runner_args: vec!["--sandbox-run".into()],
        user_memory_limit: None,
        include_dev_adapters: false,
    };
    let app = {
        let events = events.clone();
        let reg = previews.clone();
        let counter = Mutex::new(0u32);
        App::open(
            &config,
            Arc::new(move |e| events.lock().unwrap().push(e.clone())),
            Arc::new(move |dir| {
                let mut c = counter.lock().unwrap();
                *c += 1;
                let token = format!("{:032x}", *c);
                reg.lock().unwrap().insert(token.clone(), dir);
                token
            }),
            Arc::new(|_| {}),
        )
        .expect("app opens")
    };

    // Processing is refused until the self-test passes.
    let early = app.create_job("pdf-merge", &[], json!({})).unwrap_err();
    assert_eq!(early.code, "E_SANDBOX_UNAVAILABLE");
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let report = app.self_test(&home);
    assert!(report.passed, "{report:#?}");
    assert!(app.get_diagnostics().processing_enabled);

    // Files.
    let added = app
        .add_paths(vec![a.clone(), b.clone(), tmp.path().join("missing.pdf")])
        .unwrap();
    assert!(added[2].error.is_some() && added[2].file.is_none());
    let ids: Vec<String> = added[..2]
        .iter()
        .map(|r| r.file.as_ref().unwrap().file_id.clone())
        .collect();
    assert!(added[..2]
        .iter()
        .all(|r| r.file.as_ref().unwrap().actionable));

    // Targets: merge needs both; split takes one file.
    let t = app.get_targets(&ids).unwrap();
    let merge = t
        .operations
        .iter()
        .find(|o| o.adapter == "pdf-merge")
        .unwrap();
    assert!(merge.unavailable.is_none());
    let split = t
        .operations
        .iter()
        .find(|o| o.adapter == "pdf-split")
        .unwrap();
    assert!(split.unavailable.is_some());

    // Inspect, preview and page text through the file worker.
    let info = app.inspect_file(&ids[1]).unwrap();
    assert_eq!(info.pages, 3);
    let started = Instant::now();
    let p = app.request_preview(&ids[1], 2, 72).unwrap();
    let again = Instant::now();
    app.request_preview(&ids[1], 3, 72).unwrap();
    let warm = again.elapsed();
    let (token, page) = p.path.split_once('/').unwrap();
    let png = previews.lock().unwrap()[token].join(page);
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    assert_eq!((p.width_px, p.height_px), (612, 792));
    println!(
        "preview: first {:?}, warm {:?}",
        again.duration_since(started),
        warm
    );
    let text = app.get_page_text(&ids[1], 2).unwrap();
    assert!(text.words.to_string().contains("Page"));

    // A merge job, in the order given by the fileOrder param.
    let order = json!([ids[1], ids[0]]);
    let job = app
        .create_job(
            "pdf-merge",
            &ids,
            json!({"order": order, "bookmarks": "perFile"}),
        )
        .unwrap();
    let done = wait_terminal(&app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(done.outputs.len(), 1);
    assert!(
        !app.get_snapshot()
            .jobs
            .iter()
            .any(|j| j.state == JobState::Staging),
        "file-worker jobs must be hidden"
    );
    {
        let ev = events.lock().unwrap();
        assert!(ev
            .iter()
            .any(|e| e.job_id == job && e.state == JobState::Succeeded));
        assert!(ev.windows(2).all(|w| w[0].seq < w[1].seq));
    }
    // Preview of the output, then save it.
    app.request_preview(&format!("{}:0", job.as_str()), 1, 48)
        .unwrap();
    assert_eq!(app.suggested_name(&job).unwrap(), "merged.pdf");
    let dest = tmp.path().join("saved").join("merged.pdf");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let saved = app
        .save_outputs(&job, &SaveDestination::File(dest.clone()), false)
        .unwrap();
    assert_eq!(saved, 1);
    assert!(std::fs::read(&dest).unwrap().starts_with(b"%PDF"));
    assert!(app.get_snapshot().jobs.iter().all(|j| j.job_id != job));

    // Protect: the form's passwords must travel as secrets, and work.
    let job = app
        .create_job(
            "pdf-protect",
            &ids[..1],
            json!({"mode": "protect", "userPassword": "pässwörd"}),
        )
        .unwrap();
    let done = wait_terminal(&app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    let out = tmp.path().join("protected.pdf");
    app.save_outputs(&job, &SaveDestination::File(out.clone()), false)
        .unwrap();
    let check = Command::new(python)
        .args([
            "-I",
            "-c",
            &format!(
                "import pikepdf\n\
                 try:\n    pikepdf.open({p:?})\n    raise SystemExit(1)\n\
                 except pikepdf.PasswordError: pass\n\
                 assert len(pikepdf.open({p:?}, password='pässwörd').pages) == 2",
                p = out.to_str().unwrap()
            ),
        ])
        .status()
        .unwrap();
    assert!(
        check.success(),
        "the output needs exactly the given password"
    );

    // Opening many files keeps at most MAX_LIVE_FILE_WORKERS workers alive,
    // and an evicted file still works (its worker restarts).
    let many: Vec<PathBuf> = (0..6)
        .map(|i| {
            let p = tmp.path().join(format!("many-{i}.pdf"));
            make_pdf(python, &p, 1);
            p
        })
        .collect();
    let added = app.add_paths(many).unwrap();
    let many_ids: Vec<String> = added
        .iter()
        .map(|r| r.file.as_ref().unwrap().file_id.clone())
        .collect();
    for id in &many_ids {
        assert_eq!(app.inspect_file(id).unwrap().pages, 1);
        assert!(app.live_file_workers() <= converter_app_core::MAX_LIVE_FILE_WORKERS);
    }
    assert_eq!(app.inspect_file(&many_ids[0]).unwrap().pages, 1);
    for id in &many_ids {
        app.remove_file(id).unwrap();
    }

    // Bad params are refused before anything runs.
    let bad = app
        .create_job("pdf-split", &ids[..1], json!({"mode": "nonsense"}))
        .unwrap_err();
    assert_eq!(bad.code, "E_PARAMS_INVALID");

    // Exit: workers stop and every job directory goes.
    app.remove_file(&ids[1]).unwrap();
    app.shutdown();
    let jobs = tmp.path().join("data").join("jobs");
    let left: Vec<_> = std::fs::read_dir(&jobs)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "left behind: {left:?}");
}
