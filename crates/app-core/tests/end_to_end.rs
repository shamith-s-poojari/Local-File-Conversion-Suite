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
    let python = if cfg!(windows) {
        repo().join(".venv/Scripts/python.exe")
    } else {
        repo().join(".venv/bin/python")
    };
    if !python.is_file() || !cfg!(any(target_os = "macos", windows)) {
        println!("end_to_end: skipped (needs `uv sync`, and macOS or Windows until Linux CI proves bwrap)");
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
    let home = PathBuf::from(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .unwrap(),
    );
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

    conversions(&app, python, tmp.path());

    // Exit: workers stop and every job directory goes.
    app.remove_file(&ids[1]).unwrap();
    app.shutdown();
    let jobs = tmp.path().join("data").join("jobs");
    let left: Vec<_> = std::fs::read_dir(&jobs)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "left behind: {left:?}");
}

/// ADR 0003: conversions through the sandbox, image inspection and previews.
fn conversions(app: &App, python: &Path, dir: &Path) {
    let png = dir.join("photo.png");
    let ok = Command::new(python)
        .args([
            "-I",
            "-c",
            &format!(
                "from PIL import Image; Image.new('RGBA', (300, 200), (200, 30, 30, 128)).save({:?})",
                png.to_str().unwrap()
            ),
        ])
        .status()
        .unwrap()
        .success();
    assert!(ok, "making a test PNG");
    let pdf = dir.join("pages.pdf");
    make_pdf(python, &pdf, 2);
    let added = app.add_paths(vec![png, pdf]).unwrap();
    let png_id = added[0].file.as_ref().unwrap().file_id.clone();
    let pdf_id = added[1].file.as_ref().unwrap().file_id.clone();
    assert!(added[0].file.as_ref().unwrap().actionable);

    // An image is inspected and previewed by image-convert's worker.
    let info = app.inspect_file(&png_id).unwrap();
    let media = info.media.expect("image facts");
    assert_eq!(media["width"], 300);
    assert_eq!(media["kind"], "image");
    let preview = app.request_preview(&png_id, 1, 72).unwrap();
    assert!(preview.width_px > 0);

    // Targets: PNG offers JPG (lossy) and WebP; PDF offers PNG pages.
    let t = app.get_targets(std::slice::from_ref(&png_id)).unwrap();
    let to: Vec<&str> = t.groups[0]
        .conversions
        .iter()
        .map(|c| c.to.as_str())
        .collect();
    for f in ["jpg", "webp", "pdf", "gif", "tiff", "avif", "ico", "bmp"] {
        assert!(to.contains(&f), "png → {f} missing from {to:?}");
    }
    assert!(!to.contains(&"png"));

    // PNG → JPG.
    let job = app
        .create_conversion(std::slice::from_ref(&png_id), "jpg", json!({"quality": 80}))
        .unwrap();
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(app.suggested_name(&job).unwrap(), "photo.jpg");
    let preview = app
        .request_preview(&format!("{}:0", job.as_str()), 1, 72)
        .unwrap();
    assert!(preview.width_px > 0);
    app.discard_job(&job).unwrap();

    // PDF → PNG, one per page.
    let job = app
        .create_conversion(std::slice::from_ref(&pdf_id), "png", json!({"dpi": 72}))
        .unwrap();
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(done.outputs.len(), 2);
    assert_eq!(done.outputs[1].suggested_name, "pages-page-2.png");
    app.discard_job(&job).unwrap();

    // A pair nothing offers is refused.
    let bad = app
        .create_conversion(std::slice::from_ref(&png_id), "mp3", json!({}))
        .unwrap_err();
    assert_eq!(bad.code, "E_INTERNAL");

    app.remove_file(&png_id).unwrap();
    app.remove_file(&pdf_id).unwrap();

    tool_steps(app, python, dir);
    media_steps(app, python, dir);
    document_steps(app, python, dir);
    safety_steps(app, python, dir);
}

/// Runs a tool on files; returns the outputs' suggested names.
fn run_tool(app: &App, ids: &[String], tool: &str, params: serde_json::Value) -> Vec<String> {
    let t = app.get_targets(ids).unwrap();
    assert!(
        t.operations.iter().any(|o| o.adapter == tool),
        "{tool} not offered: {:?}",
        t.operations
    );
    let job = app
        .create_job(tool, ids, params)
        .unwrap_or_else(|e| panic!("{tool}: {e:?}"));
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{tool}: {:?}", done.error);
    let names = done
        .outputs
        .iter()
        .map(|o| o.suggested_name.clone())
        .collect();
    app.discard_job(&job).unwrap();
    names
}

/// Makes files with a Python snippet; `{dir}` is the test folder, written
/// with forward slashes so a Windows path's `\U` isn't read as an escape.
fn python_make(python: &Path, dir: &Path, code: &str) {
    let dir = dir.to_str().unwrap().replace('\\', "/");
    let ok = Command::new(python)
        .args(["-I", "-c", &code.replace("{dir}", &dir)])
        .status()
        .unwrap()
        .success();
    assert!(ok, "making test files: {code}");
}

fn add(app: &App, paths: Vec<PathBuf>) -> Vec<String> {
    app.add_paths(paths)
        .unwrap()
        .into_iter()
        .map(|a| a.file.expect("added").file_id)
        .collect()
}

/// The tools for images, data and PDFs (ADR 0004), in the sandbox.
fn tool_steps(app: &App, python: &Path, dir: &Path) {
    python_make(
        python,
        dir,
        "from PIL import Image\n\
         Image.new('RGB', (300, 200), (200, 30, 30)).save('{dir}/red.jpg', quality=90)\n\
         Image.new('RGBA', (300, 200), (30, 30, 200, 128)).save('{dir}/blue.png')",
    );
    let ids = add(app, vec![dir.join("red.jpg"), dir.join("blue.png")]);

    // Each file keeps its format: one part per format.
    let names = run_tool(app, &ids, "image-edit", json!({"rotate": 90}));
    assert_eq!(names, ["red-edited.jpg", "blue-edited.png"]);
    let names = run_tool(app, &ids, "image-compress", json!({"targetKb": 100}));
    assert_eq!(names, ["red-compressed.jpg", "blue-compressed.png"]);
    let names = run_tool(app, &ids, "images-to-gif", json!({"frameMs": 200}));
    assert_eq!(names, ["animation.gif"]);
    for id in &ids {
        app.remove_file(id).unwrap();
    }

    // Data: a CSV and a workbook combined (the format is chosen), then split.
    python_make(
        python,
        dir,
        "import openpyxl\n\
         open('{dir}/north.csv', 'w').write('city,sales\\nOslo,3\\nBergen,4\\n')\n\
         wb = openpyxl.Workbook(); ws = wb.active\n\
         ws.append(['sales', 'city']); ws.append([5, 'Rome']); ws.append([6, 'Oslo'])\n\
         wb.save('{dir}/south.xlsx')",
    );
    let ids = add(app, vec![dir.join("north.csv"), dir.join("south.xlsx")]);
    let names = run_tool(app, &ids, "data-combine", json!({"to": "csv"}));
    assert_eq!(names, ["combined.csv"]);
    let names = run_tool(
        app,
        &ids,
        "data-combine",
        json!({"to": "xlsx", "mode": "sheets"}),
    );
    assert_eq!(names, ["combined.xlsx"]);
    let names = run_tool(
        app,
        &ids,
        "data-split",
        json!({"mode": "column", "column": "city"}),
    );
    assert_eq!(
        names,
        [
            "north-Oslo.csv",
            "north-Bergen.csv",
            "south-Rome.xlsx",
            "south-Oslo.xlsx"
        ]
    );
    for id in &ids {
        app.remove_file(id).unwrap();
    }

    // PDF: compress and number.
    let pdf = dir.join("memo.pdf");
    make_pdf(python, &pdf, 3);
    let ids = add(app, vec![pdf]);
    let names = run_tool(app, &ids, "pdf-compress", json!({"level": "strong"}));
    assert_eq!(names, ["memo-compressed.pdf"]);
    let names = run_tool(app, &ids, "pdf-page-numbers", json!({"pages": "2-3"}));
    assert_eq!(names, ["memo-numbered.pdf"]);
    app.remove_file(&ids[0]).unwrap();
}

/// What a file reveals, removing it, making a file safe, contact sheets and
/// duplicates (ADR 0005), in the sandbox.
fn safety_steps(app: &App, python: &Path, dir: &Path) {
    python_make(
        python,
        dir,
        "import shutil\n\
         from PIL import Image\n\
         import pikepdf\n\
         from pikepdf import Dictionary, Name, String\n\
         im = Image.new('RGB', (64, 48), (10, 120, 200))\n\
         ex = Image.Exif()\n\
         ex[0x010F] = 'Maker'; ex[0x0110] = 'Phone 9'\n\
         ex.get_ifd(0x8825).update({1: 'N', 2: (59.0, 54.0, 0.0), 3: 'E', 4: (10.0, 45.0, 0.0)})\n\
         im.save('{dir}/trip.jpg', exif=ex, quality=90)\n\
         shutil.copy('{dir}/trip.jpg', '{dir}/trip copy.jpg')\n\
         pdf = pikepdf.new()\n\
         pdf.add_blank_page(); pdf.add_blank_page()\n\
         pdf.Root.OpenAction = pdf.make_indirect(Dictionary(S=Name.JavaScript, JS=String('app.alert(1)')))\n\
         pdf.save('{dir}/form.pdf')",
    );
    let kinds = |id: &str| -> Vec<String> {
        app.inspect_file(id)
            .unwrap()
            .findings
            .into_iter()
            .map(|f| f.kind)
            .collect()
    };

    // A photo reveals where and with what it was taken; cleaning removes it.
    let ids = add(app, vec![dir.join("trip.jpg"), dir.join("trip copy.jpg")]);
    assert_eq!(app.duplicates(&ids), 1);
    let found = kinds(&ids[0]);
    assert!(found.iter().any(|k| k == "location"), "{found:?}");
    assert!(found.iter().any(|k| k == "device"), "{found:?}");
    let t = app.get_targets(&ids[..1]).unwrap();
    assert!(t.operations.iter().any(|o| o.adapter == "file-clean"));
    let job = app.create_job("file-clean", &ids[..1], json!({})).unwrap();
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(done.outputs[0].suggested_name, "trip-clean.jpg");
    // Copying a JPEG: its sandboxed worker hands back a PNG.
    let png = app.clipboard_png(&job, 0).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let out = dir.join("cleaned");
    std::fs::create_dir_all(&out).unwrap();
    app.save_outputs(&job, &SaveDestination::Folder(out.clone()), false)
        .unwrap();
    let cleaned = add(app, vec![out.join("trip-clean.jpg")]);
    let left = kinds(&cleaned[0]);
    assert!(
        !left.iter().any(|k| k == "location" || k == "device"),
        "{left:?}"
    );

    // A PDF that runs JavaScript when opened is made safe.
    let pdf = add(app, vec![dir.join("form.pdf")]);
    assert!(kinds(&pdf[0]).iter().any(|k| k == "activeContent"));
    let names = run_tool(app, &pdf, "file-disarm", json!({}));
    assert_eq!(names, ["form-safe.pdf"]);

    // Redaction's verifier is a second Python process, like disarm's: both
    // must start inside the sandbox (Windows counts its processes).
    let secret = dir.join("secret.pdf");
    make_pdf(python, &secret, 2);
    let redact = add(app, vec![secret]);
    let names = run_tool(app, &redact, "pdf-redact", json!({"terms": ["Page 2"]}));
    assert_eq!(names.len(), 1, "{names:?}");
    app.remove_file(&redact[0]).unwrap();

    // A contact sheet of a photo and a PDF's pages.
    let both = [ids[0].clone(), pdf[0].clone()];
    let names = run_tool(app, &both, "contact-sheet", json!({"to": "png"}));
    assert_eq!(names, ["contact-sheet.png"]);

    for id in ids.iter().chain(&cleaned).chain(&pdf) {
        app.remove_file(id).unwrap();
    }
}

/// FFmpeg inside the sandbox: only adapters that list it get the grant.
fn media_steps(app: &App, python: &Path, dir: &Path) {
    if !app.get_diagnostics().engines.iter().any(|e| e == "ffmpeg") {
        println!("end_to_end: FFmpeg not installed, media steps skipped");
        return;
    }
    let wav = dir.join("tone.wav");
    let ok = Command::new(python)
        .args([
            "-I",
            "-c",
            &format!(
                "import math, struct, wave\n\
                 w = wave.open({:?}, 'wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(44100)\n\
                 w.writeframes(b''.join(struct.pack('<h', int(9000*math.sin(n/10))) for n in range(88200)))\n\
                 w.close()",
                wav.to_str().unwrap()
            ),
        ])
        .status()
        .unwrap()
        .success();
    assert!(ok, "making a test WAV");
    let added = app.add_paths(vec![wav]).unwrap();
    let id = added[0].file.as_ref().unwrap().file_id.clone();
    let info = app.inspect_file(&id).unwrap();
    let media = info.media.expect("audio facts");
    assert_eq!(media["kind"], "audio");
    assert_eq!(media["sampleRate"], 44100);
    let waveform = app.request_preview(&id, 1, 72).unwrap();
    assert_eq!(waveform.width_px, 576);

    let job = app
        .create_conversion(std::slice::from_ref(&id), "mp3", json!({"bitrate": 128}))
        .unwrap();
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(app.suggested_name(&job).unwrap(), "tone.mp3");
    let preview = app.request_preview(&format!("{}:0", job.as_str()), 1, 72);
    assert!(preview.is_ok(), "output waveform: {preview:?}");
    app.discard_job(&job).unwrap();

    // The MP3 back to video isn't offered; to FLAC is.
    let t = app.get_targets(std::slice::from_ref(&id)).unwrap();
    let to: Vec<&str> = t.groups[0]
        .conversions
        .iter()
        .map(|c| c.to.as_str())
        .collect();
    assert!(to.contains(&"flac") && !to.contains(&"mp4"), "{to:?}");

    // A song and a video to MP3 together: audio-convert and video-convert
    // as two parts of one job.
    let mp4 = dir.join("clip.mp4");
    let made = ffmpeg_path().is_some_and(|ffmpeg| {
        Command::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x64:rate=10:duration=1",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=duration=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
            ])
            .args(["yuv420p", "-c:a", "aac", "-shortest"])
            .arg(&mp4)
            .status()
            .is_ok_and(|s| s.success())
    });
    if made {
        let video = app.add_paths(vec![mp4]).unwrap()[0]
            .file
            .as_ref()
            .unwrap()
            .file_id
            .clone();
        assert_eq!(
            convert_all(app, &[id.clone(), video.clone()], "mp3"),
            ["tone.mp3", "clip.mp3"]
        );

        // The audio and video tools (ADR 0004).
        let names = run_tool(
            app,
            std::slice::from_ref(&id),
            "audio-edit",
            json!({"volume": -6}),
        );
        assert_eq!(names, ["tone-edited.wav"]);
        let mixed = app.get_targets(&[id.clone(), video.clone()]).unwrap();
        assert!(
            !mixed.operations.iter().any(|o| o.adapter == "video-frames"),
            "a song isn't a video"
        );
        std::fs::copy(dir.join("tone.wav"), dir.join("tone2.wav")).unwrap();
        std::fs::copy(dir.join("clip.mp4"), dir.join("clip2.mp4")).unwrap();
        let more = add(app, vec![dir.join("tone2.wav"), dir.join("clip2.mp4")]);
        let names = run_tool(
            app,
            &[id.clone(), more[0].clone()],
            "audio-join",
            json!({"to": "mp3", "gap": 1}),
        );
        assert_eq!(names, ["joined.mp3"]);
        let clips = [video.clone(), more[1].clone()];
        let names = run_tool(app, &clips, "video-join", json!({"to": "mp4"}));
        assert_eq!(names, ["joined.mp4"]);
        let names = run_tool(app, &clips[..1], "video-edit", json!({"rotate": 90}));
        assert_eq!(names, ["clip-edited.mp4"]);
        let names = run_tool(
            app,
            &clips[..1],
            "video-frames",
            json!({"to": "jpg", "count": 3}),
        );
        assert_eq!(names.len(), 3);
        for f in more {
            app.remove_file(&f).unwrap();
        }
        app.remove_file(&video).unwrap();
    } else {
        println!("end_to_end: couldn't make a test video, mixed media step skipped");
    }
    app.remove_file(&id).unwrap();
}

/// FFmpeg on PATH or where Homebrew puts it.
fn ffmpeg_path() -> Option<PathBuf> {
    let name = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ])
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

/// Runs one conversion to success and returns the outputs' suggested names.
/// The core re-sniffs every output, so success also means each one is
/// recognised as the format it claims to be.
fn convert(app: &App, id: &str, to: &str, params: serde_json::Value) -> Vec<String> {
    let job = app
        .create_conversion(&[id.to_owned()], to, params)
        .unwrap_or_else(|e| panic!("→ {to}: {e:?}"));
    let done = wait_terminal(app, &job);
    assert_eq!(done.state, JobState::Succeeded, "→ {to}: {:?}", done.error);
    let names = done
        .outputs
        .iter()
        .map(|o| o.suggested_name.clone())
        .collect();
    app.discard_job(&job).unwrap();
    names
}

/// Converts several files in one job; returns the outputs' suggested names.
fn convert_all(app: &App, ids: &[String], to: &str) -> Vec<String> {
    let t = app.get_targets(ids).unwrap();
    assert!(
        t.shared.iter().any(|s| s.to.as_str() == to),
        "all → {to} not offered: {:?}",
        t.shared
    );
    let job = app
        .create_conversion(ids, to, json!({}))
        .unwrap_or_else(|e| panic!("all → {to}: {e:?}"));
    let done = wait_terminal(app, &job);
    assert_eq!(
        done.state,
        JobState::Succeeded,
        "all → {to}: {:?}",
        done.error
    );
    let names = done
        .outputs
        .iter()
        .map(|o| o.suggested_name.clone())
        .collect();
    app.discard_job(&job).unwrap();
    names
}

fn targets_of(app: &App, id: &str) -> Vec<String> {
    app.get_targets(&[id.to_owned()]).unwrap().groups[0]
        .conversions
        .iter()
        .filter(|c| c.unavailable.is_none())
        .map(|c| c.to.as_str().to_owned())
        .collect()
}

/// Documents: Markdown, PDF to text formats, and LibreOffice inside the sandbox.
fn document_steps(app: &App, python: &Path, dir: &Path) {
    let code = format!(
        "import docx, openpyxl\n\
         d = docx.Document(); d.add_heading('Quarterly report', 1); d.add_paragraph('Revenue grew in Lisbon.')\n\
         d.save({docx:?})\n\
         wb = openpyxl.Workbook(); ws = wb.active; ws.title = 'Sales'; ws.append(['Region', 'Q1']); ws.append(['North', 1200])\n\
         wb.create_sheet('Notes').append(['Euros'])\n\
         wb.save({xlsx:?})\n\
         open({md:?}, 'w').write('# Notes\\n\\nSome *text* and a table:\\n\\n| a | b |\\n|---|---|\\n| 1 | 2 |\\n')\n\
         open({csv:?}, 'w').write('name,score\\nAda,3\\nBob,=1+1\\n')\n\
         open({yaml:?}, 'w').write('people:\\n  - name: Ada\\n    tags: [x, y]\\n')\n",
        docx = dir.join("report.docx").to_str().unwrap(),
        xlsx = dir.join("book.xlsx").to_str().unwrap(),
        md = dir.join("notes.md").to_str().unwrap(),
        csv = dir.join("scores.csv").to_str().unwrap(),
        yaml = dir.join("team.yaml").to_str().unwrap(),
    );
    let ok = Command::new(python)
        .args(["-I", "-c", &code])
        .status()
        .unwrap()
        .success();
    assert!(ok, "making test documents");
    let pdf = dir.join("letter.pdf");
    make_pdf(python, &pdf, 2);
    let added = app
        .add_paths(vec![
            dir.join("notes.md"),
            pdf,
            dir.join("report.docx"),
            dir.join("book.xlsx"),
            dir.join("scores.csv"),
            dir.join("team.yaml"),
        ])
        .unwrap();
    let ids: Vec<String> = added
        .iter()
        .map(|a| a.file.as_ref().unwrap().file_id.clone())
        .collect();
    let (md, pdf, docx, xlsx) = (&ids[0], &ids[1], &ids[2], &ids[3]);
    let (csv, yaml) = (&ids[4], &ids[5]);

    // Markdown and PDF need no LibreOffice.
    let to = targets_of(app, md);
    for f in ["pdf", "html", "txt"] {
        assert!(to.contains(&f.to_owned()), "md → {f} missing from {to:?}");
    }
    assert_eq!(
        convert(app, md, "pdf", json!({"pageSize": "letter"})),
        ["notes.pdf"]
    );
    assert_eq!(convert(app, md, "html", json!({})), ["notes.html"]);
    assert_eq!(convert(app, md, "txt", json!({})), ["notes.txt"]);
    let to = targets_of(app, pdf);
    for f in ["docx", "xlsx", "md", "html", "txt", "png"] {
        assert!(to.contains(&f.to_owned()), "pdf → {f} missing from {to:?}");
    }
    assert_eq!(convert(app, pdf, "docx", json!({})), ["letter.docx"]);
    assert_eq!(convert(app, pdf, "md", json!({})), ["letter.md"]);
    assert_eq!(
        convert(app, pdf, "txt", json!({"pages": "2"})),
        ["letter.txt"]
    );

    // Tables and data need no LibreOffice either.
    for (id, wanted) in [
        (csv, &["json", "yaml", "xlsx", "md", "html"][..]),
        (yaml, &["json", "csv", "xlsx"][..]),
        (xlsx, &["json", "yaml"][..]),
    ] {
        let to = targets_of(app, id);
        for f in wanted {
            assert!(to.contains(&(*f).to_owned()), "{f} missing from {to:?}");
        }
    }
    assert_eq!(convert(app, csv, "json", json!({})), ["scores.json"]);
    assert_eq!(convert(app, csv, "xlsx", json!({})), ["scores.xlsx"]);
    assert_eq!(convert(app, yaml, "xlsx", json!({})), ["team.xlsx"]);
    assert_eq!(convert(app, xlsx, "yaml", json!({})), ["book.yaml"]);
    assert_eq!(
        convert(app, xlsx, "csv", json!({})),
        ["book-Sales.csv", "book-Notes.csv"]
    );

    // A mixed selection runs as one job in parts, one per adapter.
    assert_eq!(
        convert_all(app, &[md.clone(), csv.clone()], "html"),
        ["notes.html", "scores.html"]
    );

    if app
        .get_diagnostics()
        .engines
        .iter()
        .any(|e| e == "libreoffice")
    {
        // LibreOffice runs inside the per-job sandbox with only its grant.
        assert_eq!(convert(app, docx, "pdf", json!({})), ["report.pdf"]);
        assert_eq!(convert(app, docx, "md", json!({})), ["report.md"]);
        assert_eq!(convert(app, docx, "odt", json!({})), ["report.odt"]);
        assert_eq!(convert(app, xlsx, "png", json!({"dpi": 72})).len(), 2);
        // Word (LibreOffice) and Markdown (markup-convert) together.
        assert_eq!(
            convert_all(app, &[docx.clone(), md.clone()], "pdf"),
            ["report.pdf", "notes.pdf"]
        );
        assert_eq!(convert(app, md, "docx", json!({})), ["notes.docx"]);
        assert_eq!(convert(app, pdf, "pptx", json!({})), ["letter.pptx"]);
    } else {
        println!("end_to_end: LibreOffice not installed, office steps skipped");
        assert!(!targets_of(app, docx).contains(&"pdf".to_owned()));
    }
    for id in &ids {
        app.remove_file(id).unwrap();
    }
}
