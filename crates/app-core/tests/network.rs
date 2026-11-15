//! Network test (spec 14.5, change C4), the part that needs no root.
//!
//! Runs every tool, inspection, preview and page-text path on a corpus whose
//! links, actions, scripts, attachments, metadata and external streams all
//! point at a local **canary listener**, while a sampler lists every socket
//! held by the core's child processes (runners, workers, verifiers).
//!
//! It fails if:
//! - the canary receives any connection (item 6), or
//! - any child process holds an internet socket (item 3, for workers).
//!
//! Positive controls (item 5) make a harness that sees nothing fail instead
//! of pass: the listener must count a deliberate connection, the sampler must
//! catch a child that holds one, and the sampler must actually see workers.
//!
//! Packet capture with per-process attribution, which also covers the app's
//! own process and the WebView, is `tools/network-test/capture.sh` (needs
//! root; nightly). A plain `main` (`harness = false`): started with
//! `--sandbox-run` it is the worker runner, like the app.

use std::collections::BTreeSet;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use converter_app_core::{App, AppConfig};
use converter_jobstore::{JobId, JobState};
use serde_json::{json, Value};

const SAMPLE_EVERY: Duration = Duration::from_millis(100);

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
        println!("network: skipped (needs `uv sync`, and macOS until Linux CI proves bwrap)");
        return ExitCode::SUCCESS;
    }
    run(&python);
    println!("network: ok");
    ExitCode::SUCCESS
}

// ------------------------------------------------------------------ canary

/// Counts every connection, and keeps what was asked for (for the report).
struct Canary {
    port: u16,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Canary {
    fn start() -> Canary {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (h, r) = (hits.clone(), requests.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                h.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                let mut buf = [0u8; 256];
                let n = stream.read(&mut buf).unwrap_or(0);
                let first = String::from_utf8_lossy(&buf[..n]);
                r.lock()
                    .unwrap()
                    .push(first.lines().next().unwrap_or("").to_owned());
            }
        });
        Canary {
            port,
            hits,
            requests,
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.hits.store(0, Ordering::SeqCst);
        self.requests.lock().unwrap().clear();
    }
}

// ----------------------------------------------------------------- sampler

/// Lists internet sockets held by this process's descendants.
struct Sampler {
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<BTreeSet<String>>>,
    /// Distinct descendant processes seen (positive control: workers exist).
    seen: Arc<Mutex<BTreeSet<String>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Sampler {
    fn start() -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let sockets: Arc<Mutex<BTreeSet<String>>> = Arc::default();
        let seen: Arc<Mutex<BTreeSet<String>>> = Arc::default();
        let (s, k, v) = (stop.clone(), sockets.clone(), seen.clone());
        let handle = thread::spawn(move || {
            let me = std::process::id();
            while !s.load(Ordering::SeqCst) {
                let kids = descendants(me);
                if !kids.is_empty() {
                    {
                        let mut v = v.lock().unwrap();
                        for (pid, comm) in &kids {
                            v.insert(format!("{pid} {comm}"));
                        }
                    }
                    for line in inet_sockets(&kids) {
                        k.lock().unwrap().insert(line);
                    }
                }
                thread::sleep(SAMPLE_EVERY);
            }
        });
        Sampler {
            stop,
            sockets,
            seen,
            handle: Some(handle),
        }
    }

    fn sockets(&self) -> BTreeSet<String> {
        self.sockets.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.sockets.lock().unwrap().clear();
        self.seen.lock().unwrap().clear();
    }

    fn workers_seen(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("ython") || p.contains("sandbox"))
            .count()
    }

    fn finish(mut self) -> BTreeSet<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            h.join().unwrap();
        }
        self.sockets()
    }
}

/// `(pid, command)` for every process below `root`, excluding the sampler's
/// own `ps` and `lsof`.
fn descendants(root: u32) -> Vec<(u32, String)> {
    let Ok(out) = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "comm="])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let table: Vec<(u32, u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let comm = it.collect::<Vec<_>>().join(" ");
            Some((pid, ppid, comm))
        })
        .collect();
    let mut found = vec![root];
    let mut i = 0;
    while i < found.len() {
        let parent = found[i];
        for (pid, ppid, _) in &table {
            if *ppid == parent && !found.contains(pid) {
                found.push(*pid);
            }
        }
        i += 1;
    }
    table
        .into_iter()
        .filter(|(pid, _, comm)| {
            *pid != root
                && found.contains(pid)
                && !comm.ends_with("/ps")
                && !comm.ends_with("lsof")
                && comm != "ps"
        })
        .map(|(pid, _, comm)| (pid, comm))
        .collect()
}

/// `lsof` lines for internet sockets (TCP and UDP, any state) held by `pids`.
fn inet_sockets(pids: &[(u32, String)]) -> Vec<String> {
    let list = pids
        .iter()
        .map(|(p, _)| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let Ok(out) = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-i", "-a", "-p", &list, "-F", "pcn"])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let (mut pid, mut comm) = (String::new(), String::new());
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        match l.split_at(1) {
            ("p", rest) => pid = rest.to_owned(),
            ("c", rest) => comm = rest.to_owned(),
            ("n", rest) => lines.push(format!("{pid} {comm} {rest}")),
            _ => {}
        }
    }
    lines
}

// ------------------------------------------------------------------- run

fn wait_until(what: &str, limit: Duration, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !ok() {
        assert!(Instant::now() < deadline, "positive control failed: {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn finished(app: &App, job: &JobId) -> JobState {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let state = app
            .get_snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == job)
            .map(|j| (j.state, j.error))
            .expect("job listed");
        if state.0.is_terminal() {
            assert_eq!(state.0, JobState::Succeeded, "{:?}", state.1);
            return state.0;
        }
        assert!(Instant::now() < deadline, "job didn't finish");
        thread::sleep(Duration::from_millis(50));
    }
}

fn run(python: &Path) {
    let canary = Canary::start();
    let sampler = Sampler::start();

    // Control A: the listener counts a real connection.
    drop(TcpStream::connect(("127.0.0.1", canary.port)).unwrap());
    wait_until("canary counts a connection", Duration::from_secs(3), || {
        canary.hits() == 1
    });

    // Control B: the sampler catches a child process holding a socket.
    let mut holder = Command::new(python)
        .args([
            "-I",
            "-c",
            &format!(
                "import socket, time\ns = socket.create_connection(('127.0.0.1', {}))\ntime.sleep(3)",
                canary.port
            ),
        ])
        .spawn()
        .unwrap();
    wait_until(
        "sampler sees a child's socket",
        Duration::from_secs(5),
        || {
            sampler
                .sockets()
                .iter()
                .any(|s| s.contains(&format!(":{}", canary.port)))
        },
    );
    holder.kill().ok();
    holder.wait().ok();
    thread::sleep(SAMPLE_EVERY * 3);
    canary.reset();
    sampler.clear();

    // The corpus, built before the run so its generator isn't sampled.
    let tmp = tempfile::tempdir().unwrap();
    let corpus = tmp.path().join("corpus");
    let made = Command::new(python)
        .arg("-I")
        .arg(repo().join("tools/network-test/canary_corpus.py"))
        .arg(&corpus)
        .arg(canary.port.to_string())
        .output()
        .unwrap();
    assert!(made.status.success(), "building the canary corpus");
    let files: Vec<PathBuf> = String::from_utf8(made.stdout)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect();
    assert_eq!(files.len(), 4);
    thread::sleep(SAMPLE_EVERY * 3);
    sampler.clear();

    let app = App::open(
        &AppConfig {
            data_root: Some(tmp.path().join("data")),
            resources: repo(),
            adapters_dir: repo().join("adapters"),
            python: python.to_path_buf(),
            runner: std::env::current_exe().unwrap(),
            runner_args: vec!["--sandbox-run".into()],
            user_memory_limit: None,
            include_dev_adapters: false,
        },
        Arc::new(|_| {}),
        Arc::new(|_| "00000000000000000000000000000000".to_owned()),
        Arc::new(|_| {}),
    )
    .expect("app opens");
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(app.self_test(&home).passed, "sandbox self-test");

    let added = app.add_paths(files).unwrap();
    let ids: Vec<String> = added
        .iter()
        .map(|r| {
            r.file
                .as_ref()
                .expect("canary file accepted")
                .file_id
                .clone()
        })
        .collect();

    // Every read path: inspect, previews and page text, on every file.
    for id in &ids {
        app.inspect_file(id).unwrap();
        for page in [1, 2] {
            app.request_preview(id, page, 72).unwrap();
            app.get_page_text(id, page).unwrap();
        }
    }

    // Every tool.
    let jobs: Vec<(&str, Vec<String>, Value)> = vec![
        ("pdf-merge", ids.clone(), json!({"bookmarks": "perFile"})),
        (
            "pdf-split",
            vec![ids[0].clone()],
            json!({"mode": "eachPage"}),
        ),
        (
            "pdf-organize",
            vec![ids[1].clone()],
            json!({"pages": [{"source": 2}, {"source": 1, "rotate": 90}]}),
        ),
        (
            "pdf-protect",
            vec![ids[2].clone()],
            json!({"mode": "protect", "userPassword": "network-test"}),
        ),
        (
            "pdf-watermark",
            vec![ids[3].clone()],
            json!({"text": "CANARY"}),
        ),
        (
            "pdf-redact",
            vec![ids[3].clone()],
            json!({"terms": ["canary page 1"]}),
        ),
    ];
    for (tool, files, params) in jobs {
        let job = app.create_job(tool, &files, params).unwrap();
        finished(&app, &job);
        // Previewing the output is a read path too (a protected output
        // rightly needs its password, so it's skipped).
        if tool != "pdf-protect" {
            app.request_preview(&format!("{}:0", job.as_str()), 1, 48)
                .unwrap();
        }
        app.discard_job(&job).unwrap();
    }

    let workers = sampler.workers_seen();
    app.shutdown();
    thread::sleep(SAMPLE_EVERY * 3);
    let sockets = sampler.finish();

    // Control C: the sampler really watched the workers.
    assert!(
        workers > 0,
        "positive control failed: the sampler never saw a worker process"
    );
    let requests = canary.requests.lock().unwrap().clone();
    assert_eq!(
        canary.hits(),
        0,
        "the canary was contacted: {requests:?} (spec 14.5 item 6)"
    );
    assert!(
        sockets.is_empty(),
        "child processes held internet sockets: {sockets:#?} (spec 14.5 item 3)"
    );
    println!("network: {workers} worker processes watched, no sockets, no canary hits");
}
