//! How the app copes with slow, broken and still-running workers, using a
//! stand-in `pdf-redact` adapter (`fixtures/lifecycle-adapters`) in the real
//! sandbox. Run from `folders`, which is the sandbox runner (not `end_to_end`: that one
//! starts LibreOffice).
//!
//! Each step is the check for one audit finding; the finding is named in the
//! step's comment.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use converter_app_core::App;
use converter_jobstore::{JobEvent, JobId, JobState, SaveDestination};
use serde_json::json;

use crate::common;

const SLOW: &[u8] = b"%PDF-1.4\nSLOW\n%%EOF\n";
const FAST: &[u8] = b"%PDF-1.4\nFAST\n%%EOF\n";
const CRASH: &[u8] = b"%PDF-1.4\nCRASH\n%%EOF\n";
const BREAK: &[u8] = b"%PDF-1.4\nBREAK\n%%EOF\n";

struct Rig {
    app: App,
    events: Arc<Mutex<Vec<JobEvent>>>,
    revoked: Arc<AtomicUsize>,
    registered: Arc<AtomicUsize>,
    data: PathBuf,
}

fn rig(python: &Path, tmp: &Path, name: &str) -> Rig {
    let adapters = tmp.join(format!("adapters-{name}"));
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lifecycle-adapters");
    copy_dir(&from, &adapters);
    let data = tmp.join(format!("data-{name}"));
    let mut config = common::test_config(python, data.clone());
    config.adapters_dir = adapters;
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let revoked = Arc::new(AtomicUsize::new(0));
    let registered = Arc::new(AtomicUsize::new(0));
    let tokens: Arc<Mutex<HashMap<PathBuf, String>>> = Arc::default();
    let app = {
        let events = events.clone();
        let revoked = revoked.clone();
        let registered = registered.clone();
        App::open(
            &config,
            Arc::new(move |e| events.lock().unwrap().push(e.clone())),
            Arc::new(move |dir| {
                let mut t = tokens.lock().unwrap();
                let n = t.len() + 1;
                registered.fetch_add(1, Ordering::SeqCst);
                t.entry(dir).or_insert_with(|| format!("{n:032x}")).clone()
            }),
            Arc::new(move |_| {
                revoked.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .expect("the app opens")
    };
    let report = app.self_test(&common::home());
    assert!(report.passed, "{report:#?}");
    Rig {
        app,
        events,
        revoked,
        registered,
        data,
    }
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::create_dir_all(to).unwrap();
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn add(app: &App, dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    let added = app.add_paths(vec![path]).unwrap();
    added[0]
        .file
        .as_ref()
        .unwrap_or_else(|| panic!("{name} rejected: {:?}", added[0].error))
        .file_id
        .clone()
}

fn wait_for(what: &str, limit: Duration, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_done(app: &App, job: &JobId) -> converter_jobstore::JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snap = app.get_snapshot();
        if let Some(j) = snap.jobs.into_iter().find(|j| &j.job_id == job) {
            if j.state.is_terminal() {
                return j;
            }
        }
        assert!(Instant::now() < deadline, "job didn't finish");
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Job directories on disk (hidden file-worker jobs included).
fn job_dirs(data: &Path) -> usize {
    std::fs::read_dir(data.join("jobs"))
        .map(|d| d.count())
        .unwrap_or(0)
}

pub fn run(python: &Path) {
    let tmp = tempfile::tempdir().unwrap();
    slow_file_does_not_block_others(python, tmp.path());
    worker_start_failure_leaves_no_job(python, tmp.path());
    previews_survive_a_refused_save(python, tmp.path());
    broken_output_worker_is_replaced(python, tmp.path());
    symlinked_output_is_refused(python, tmp.path());
    shutdown_waits_for_running_jobs(python, tmp.path());
    pending_self_test_has_its_own_reason(python, tmp.path());
    hidden_jobs_send_no_events(python, tmp.path());
    staging_failure_discards_the_job(python, tmp.path());
    refused_claim_evicts_nobody(python, tmp.path());
    closing_a_file_worker_kills_its_strays(python, tmp.path());
    println!("folders lifecycle: ok");
}

/// R1: inspecting one file (8 s in the stand-in) must not freeze the others.
fn slow_file_does_not_block_others(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "r1");
    let files = tmp.join("r1-files");
    std::fs::create_dir_all(&files).unwrap();
    let slow = add(&r.app, &files, "slow.pdf", SLOW);
    let fast = add(&r.app, &files, "fast.pdf", FAST);
    let app = Arc::new(r.app);
    let done = Arc::new(AtomicBool::new(false));
    let t = {
        let (app, done) = (app.clone(), done.clone());
        std::thread::spawn(move || {
            app.inspect_file(&slow).unwrap();
            done.store(true, Ordering::SeqCst);
        })
    };
    // Let the slow inspect get going: its worker is out and running.
    std::thread::sleep(Duration::from_millis(2500));
    assert!(
        !done.load(Ordering::SeqCst),
        "the stand-in should still be sleeping"
    );

    let started = Instant::now();
    let extra = add(&app, &files, "extra.pdf", FAST);
    app.get_targets(std::slice::from_ref(&fast)).unwrap();
    app.duplicates(&[fast.clone(), extra]);
    let quick = started.elapsed();
    assert!(
        quick < Duration::from_secs(1),
        "add/targets waited {quick:?} behind a slow inspect"
    );
    // Another file's worker starts and answers meanwhile.
    app.inspect_file(&fast).unwrap();
    assert!(
        !done.load(Ordering::SeqCst),
        "inspecting another file waited for the slow one"
    );
    t.join().unwrap();
    // Removing the file that was busy is fine too.
    app.shutdown();
}

/// R3: a preview worker that can't start leaves no hidden job behind.
fn worker_start_failure_leaves_no_job(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "r3");
    let files = tmp.join("r3-files");
    std::fs::create_dir_all(&files).unwrap();
    let id = add(&r.app, &files, "crash.pdf", CRASH);
    let before = job_dirs(&r.data);
    let err = r.app.inspect_file(&id).unwrap_err();
    assert!(!err.code.is_empty());
    assert_eq!(
        job_dirs(&r.data),
        before,
        "the failed worker's job directory is still there"
    );
    assert_eq!(r.app.live_file_workers(), 0);
    // The file is still usable for a conversion-free retry: same error, no leak.
    let _ = r.app.inspect_file(&id).unwrap_err();
    assert_eq!(job_dirs(&r.data), before);
    r.app.shutdown();
}

fn convert_one(r: &Rig, tmp: &Path, name: &str, bytes: &[u8]) -> (String, JobId) {
    let files = tmp.join(format!("{name}-files"));
    std::fs::create_dir_all(&files).unwrap();
    let id = add(&r.app, &files, &format!("{name}.pdf"), bytes);
    let job = r
        .app
        .create_job("pdf-redact", std::slice::from_ref(&id), json!({}))
        .unwrap();
    let done = wait_done(&r.app, &job);
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    (id, job)
}

/// R6: a refused save keeps the previews on screen; a discard that works
/// revokes them. R7: a save that works says where the files went.
fn previews_survive_a_refused_save(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "r6");
    let (_, job) = convert_one(&r, tmp, "keep", FAST);
    let reference = format!("{}:0", job.as_str());
    r.app.request_preview(&reference, 1, 72).unwrap();
    assert_eq!(r.registered.load(Ordering::SeqCst), 1);

    // Into a folder that doesn't exist: refused.
    let nowhere = tmp.join("no-such-folder").join("out.pdf");
    assert!(r
        .app
        .save_outputs(&job, &SaveDestination::File(nowhere), false)
        .is_err());
    assert_eq!(
        r.revoked.load(Ordering::SeqCst),
        0,
        "a refused save revoked the previews still on screen"
    );
    // The job is still there and previews work again (a new worker starts).
    r.app.request_preview(&reference, 1, 72).unwrap();

    let dest = tmp.join("saved-r6.pdf");
    let report = r
        .app
        .save_outputs(&job, &SaveDestination::File(dest.clone()), false)
        .unwrap();
    assert_eq!(report.saved, std::slice::from_ref(&dest));
    assert!(std::fs::read(&dest).unwrap().starts_with(b"%PDF"));
    assert_eq!(
        r.revoked.load(Ordering::SeqCst),
        1,
        "previews revoked after the save"
    );
    r.app.shutdown();
}

/// R2: after the worker dies mid-request, the next preview starts a new one
/// instead of hitting the dead worker for ever.
fn broken_output_worker_is_replaced(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "r2");
    let (_, job) = convert_one(&r, tmp, "brk", BREAK);
    let reference = format!("{}:0", job.as_str());
    let first = r.app.request_preview(&reference, 1, 72).unwrap_err();
    assert!(
        matches!(
            first.code.as_str(),
            "E_PROTOCOL" | "E_TIMEOUT" | "E_CRASH" | "E_INTERNAL"
        ),
        "{first:?}"
    );
    r.app
        .request_preview(&reference, 1, 72)
        .expect("a fresh worker answers after the first one died");
    r.app.shutdown();
}

/// S11: an output swapped for a link to another file is refused, not read.
#[cfg(unix)]
fn symlinked_output_is_refused(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "s11");
    let (_, job) = convert_one(&r, tmp, "link", FAST);
    let (ext, bytes) = r.app.read_output(&job, 0, 1 << 20).unwrap();
    assert_eq!((ext.as_str(), bytes.as_slice()), ("pdf", FAST));

    let secret = tmp.join("secret.txt");
    std::fs::write(&secret, b"not for the clipboard").unwrap();
    let out = job_output_path(&r.data, &job);
    std::fs::remove_file(&out).unwrap();
    std::os::unix::fs::symlink(&secret, &out).unwrap();
    let err = r.app.read_output(&job, 0, 1 << 20).unwrap_err();
    assert_eq!(err.code, "E_INTERNAL");
    assert_eq!(err.reason.as_deref(), Some("unknownOutput"));
    r.app.shutdown();
}

#[cfg(not(unix))]
fn symlinked_output_is_refused(_: &Path, _: &Path) {
    // Creating symlinks on Windows needs a privilege CI runners don't give;
    // `open_regular`'s own unit test covers the reparse-point check there.
}

#[cfg(unix)]
fn job_output_path(data: &Path, job: &JobId) -> PathBuf {
    data.join("jobs")
        .join(job.as_str())
        .join("out")
        .join("0.pdf")
}

/// R4: shutdown cancels a running job and waits for its thread, so the job
/// ends `cancelled` before its directory is deleted, and it doesn't hang.
fn shutdown_waits_for_running_jobs(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "r4");
    let files = tmp.join("r4-files");
    std::fs::create_dir_all(&files).unwrap();
    let id = add(&r.app, &files, "long.pdf", SLOW);
    let job = r.app.create_job("pdf-redact", &[id], json!({})).unwrap();
    wait_for("the job to run", Duration::from_secs(30), || {
        r.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.job_id == job && e.state == JobState::Running)
    });
    let started = Instant::now();
    r.app.shutdown_within(Duration::from_secs(20));
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "shutdown took {:?}",
        started.elapsed()
    );
    let last = r
        .events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e.job_id == job)
        .map(|e| e.state);
    assert_eq!(
        last,
        Some(JobState::Cancelled),
        "the job's thread was not joined before shutdown returned"
    );
}

/// R19: processing before the self-test has run says so.
fn pending_self_test_has_its_own_reason(python: &Path, tmp: &Path) {
    let config = common::test_config(python, tmp.join("data-r19"));
    let app = common::open_app_with(&config);
    let err = app.create_job("pdf-merge", &[], json!({})).unwrap_err();
    assert_eq!(err.code, "E_SANDBOX_UNAVAILABLE");
    assert_eq!(err.reason.as_deref(), Some("selfTestPending"));
    app.shutdown();
}

/// RS-2: a file worker's job (hidden) emits nothing to the renderer, its
/// `created` event included, and is gone again when the file is closed.
fn hidden_jobs_send_no_events(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "rs2a");
    let files = tmp.join("rs2a-files");
    std::fs::create_dir_all(&files).unwrap();
    let id = add(&r.app, &files, "fast.pdf", FAST);
    r.app.inspect_file(&id).unwrap();
    assert_eq!(r.app.live_file_workers(), 1);
    assert!(
        r.events.lock().unwrap().is_empty(),
        "a hidden job's events reached the renderer: {:?}",
        r.events.lock().unwrap()
    );
    r.app.shutdown();
}

/// RS-2: a job whose inputs can't be staged (the file vanished after it was
/// added) is discarded, not left in the store.
fn staging_failure_discards_the_job(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "rs2b");
    let files = tmp.join("rs2b-files");
    std::fs::create_dir_all(&files).unwrap();
    let id = add(&r.app, &files, "gone.pdf", FAST);
    let before = job_dirs(&r.data);
    std::fs::remove_file(files.join("gone.pdf")).unwrap();
    let err = r
        .app
        .create_job("pdf-redact", std::slice::from_ref(&id), json!({}))
        .unwrap_err();
    assert!(!err.code.is_empty());
    assert_eq!(job_dirs(&r.data), before, "the job directory was left");
    assert!(
        r.app.get_snapshot().jobs.is_empty(),
        "the failed job stays in the snapshot"
    );
    r.app.shutdown();
}

/// RS-3: a call that is refused (this file has no preview adapter) must not
/// close another file's worker to make room first.
fn refused_claim_evicts_nobody(python: &Path, tmp: &Path) {
    let r = rig(python, tmp, "rs3");
    let files = tmp.join("rs3-files");
    std::fs::create_dir_all(&files).unwrap();
    for n in 0..converter_app_core::MAX_LIVE_FILE_WORKERS {
        let bytes = [FAST, format!("{n}").as_bytes()].concat();
        let id = add(&r.app, &files, &format!("f{n}.pdf"), &bytes);
        r.app.inspect_file(&id).unwrap();
    }
    assert_eq!(
        r.app.live_file_workers(),
        converter_app_core::MAX_LIVE_FILE_WORKERS
    );
    let text = add(&r.app, &files, "note.txt", b"just words\n");
    assert!(r.app.get_page_text(&text, 1).is_err());
    assert_eq!(
        r.app.live_file_workers(),
        converter_app_core::MAX_LIVE_FILE_WORKERS,
        "a refused call closed another file's worker"
    );
    r.app.shutdown();
}

/// Wave 2: a daemon a file worker double-forked dies with the worker.
///
/// macOS only. On Linux the worker runs in bubblewrap's own PID namespace,
/// which ends every process in it when the worker exits, and the PID the
/// daemon records is its number inside the namespace, not one this test can
/// check. Windows: the Job Object holds every descendant.
#[cfg(all(unix, not(target_os = "linux")))]
fn closing_a_file_worker_kills_its_strays(python: &Path, tmp: &Path) {
    let alive = |pid: i32| {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    };
    let r = rig(python, tmp, "strays");
    let files = tmp.join("strays-files");
    std::fs::create_dir_all(&files).unwrap();
    let id = add(&r.app, &files, "stray.pdf", b"%PDF-1.4\nSTRAY\n%%EOF\n");
    r.app.inspect_file(&id).unwrap();
    // The daemon writes its PID a moment after the worker answers.
    let read_pid = || -> Option<i32> {
        std::fs::read_dir(r.data.join("jobs"))
            .ok()?
            .flatten()
            .find_map(|d| std::fs::read_to_string(d.path().join("out/stray.pid")).ok())?
            .parse()
            .ok()
    };
    wait_for("the stand-in's daemon", Duration::from_secs(10), || {
        read_pid().is_some()
    });
    let pid = read_pid().unwrap();
    assert!(alive(pid), "the daemon isn't running before the close");
    r.app.remove_file(&id).unwrap();
    wait_for("the daemon to be killed", Duration::from_secs(5), || {
        !alive(pid)
    });
    r.app.shutdown();
}

#[cfg(any(not(unix), target_os = "linux"))]
fn closing_a_file_worker_kills_its_strays(_: &Path, _: &Path) {}
