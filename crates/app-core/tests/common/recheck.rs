//! The output re-check (ADR 0006, audit S10) through the real app and the
//! real sandbox, run from `folders` (the sandbox runner; not `end_to_end`,
//! which starts LibreOffice).
//!
//! The tool is a stand-in `pdf-redact` (`fixtures/recheck-adapters`) whose
//! worker always claims success and hands back its input. The check is the
//! real `output-check` with the real `pdf-redact/verify.py`, copied next to
//! it, so the only thing that can tell a good output from a bad one is the
//! separate check job.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use converter_app_core::App;
use converter_jobstore::{JobEvent, JobId, JobSnapshot, JobState};
use serde_json::json;

use crate::common;

const SECRET: &str = "Zorblax7731";

/// Builds a PDF with the secret in plain sight and one with a black box over
/// where it was, both without metadata (so only the text and the box differ).
const MAKE_PDFS: &str = r#"
import sys, pymupdf
from pathlib import Path
out = Path(sys.argv[1])
def build(path, leaky):
    doc = pymupdf.open()
    page = doc.new_page(width=612, height=792)
    page.insert_text((72, 300), "Visible-Keeper stays readable.", fontsize=12)
    if leaky:
        page.insert_text((72, 100), "Account Zorblax7731 belongs to the client.", fontsize=12)
    else:
        page.draw_rect(pymupdf.Rect(20, 70, 590, 140), color=(0, 0, 0), fill=(0, 0, 0))
    doc.set_metadata({})
    doc.save(path, garbage=4, deflate=True)
build(out / "leaky.pdf", True)
build(out / "clean.pdf", False)
"#;

struct Rig {
    app: App,
    events: Arc<Mutex<Vec<JobEvent>>>,
    data: PathBuf,
    files: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "__pycache__" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to.join(name));
        } else {
            std::fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}

fn rig(python: &Path, tmp: &Path, name: &str) -> Rig {
    let adapters = tmp.join(format!("adapters-{name}"));
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy_dir(&here.join("tests/fixtures/recheck-adapters"), &adapters);
    // The check is the product's own, and so is the verifier it loads from
    // the tool's folder.
    copy_dir(
        &common::repo().join("adapters/output-check"),
        &adapters.join("output-check"),
    );
    std::fs::copy(
        common::repo().join("adapters/pdf-redact/verify.py"),
        adapters.join("pdf-redact/verify.py"),
    )
    .unwrap();

    let files = tmp.join(format!("files-{name}"));
    std::fs::create_dir_all(&files).unwrap();
    let made = std::process::Command::new(python)
        .args(["-I", "-c", MAKE_PDFS])
        .arg(&files)
        .status()
        .unwrap();
    assert!(made.success(), "building the test PDFs");

    let data = tmp.join(format!("data-{name}"));
    let mut config = common::test_config(python, data.clone());
    config.adapters_dir = adapters;
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let sink = events.clone();
    let app = App::open(
        &config,
        Arc::new(move |e| sink.lock().unwrap().push(e.clone())),
        Arc::new(|_| "00000000000000000000000000000000".to_owned()),
        Arc::new(|_| {}),
    )
    .expect("the app opens");
    let report = app.self_test(&common::home());
    assert!(report.passed, "{report:#?}");
    Rig {
        app,
        events,
        data,
        files,
    }
}

impl Rig {
    fn add(&self, name: &str) -> String {
        let added = self.app.add_paths(vec![self.files.join(name)]).unwrap();
        added[0]
            .file
            .as_ref()
            .unwrap_or_else(|| panic!("{name} rejected: {:?}", added[0].error))
            .file_id
            .clone()
    }

    fn redact(&self, name: &str) -> JobSnapshot {
        let id = self.add(name);
        let params = json!({
            "terms": [SECRET],
            "boxes": [{"page": 1, "x0": 0.05, "y0": 0.1, "x1": 0.95, "y1": 0.16}],
        });
        let job = self.app.create_job("pdf-redact", &[id], params).unwrap();
        self.wait_done(&job)
    }

    fn wait_done(&self, job: &JobId) -> JobSnapshot {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let found = self
                .app
                .get_snapshot()
                .jobs
                .into_iter()
                .find(|j| &j.job_id == job);
            if let Some(j) = found.filter(|j| j.state.is_terminal()) {
                return j;
            }
            assert!(Instant::now() < deadline, "job didn't finish");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// States this job went through, from the events the UI would get.
    fn states(&self, job: &JobId) -> Vec<JobState> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| &e.job_id == job)
            .map(|e| e.state)
            .collect()
    }

    /// Check-job folders anywhere under the data folder.
    fn leftover_checks(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if e.file_name().to_string_lossy().starts_with("recheck-") {
                    out.push(p.clone());
                }
                if p.is_dir() {
                    walk(&p, out);
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.data, &mut found);
        found
    }
}

pub fn run(python: &Path) {
    let tmp = tempfile::tempdir().unwrap();
    let r = rig(python, tmp.path(), "s10");
    unredacted_output_fails_though_its_worker_claimed_success(&r);
    properly_redacted_output_succeeds(&r);
    the_check_adapter_is_not_a_tool(&r);
    r.app.shutdown();
    println!("folders recheck: ok");
}

/// S10: the tool's worker says "done" over a file that still holds the
/// secret. Without the core's own check the job would succeed.
fn unredacted_output_fails_though_its_worker_claimed_success(r: &Rig) {
    let job = r.redact("leaky.pdf");
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    assert_eq!(
        job.error.as_ref().map(|e| e.code.as_str()),
        Some("E_REDACTION_VERIFICATION_FAILED"),
        "{job:?}"
    );
    assert!(job.outputs.is_empty(), "the failed output is deleted");
    // It got as far as verifying: the worker's own run was fine.
    let states = r.states(&job.job_id);
    assert!(states.contains(&JobState::Running), "{states:?}");
    assert!(states.contains(&JobState::Verifying), "{states:?}");
    assert!(!states.contains(&JobState::Succeeded), "{states:?}");
}

fn properly_redacted_output_succeeds(r: &Rig) {
    let job = r.redact("clean.pdf");
    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    assert_eq!(job.outputs.len(), 1);
    // Verifying, then done: the check happens in between and is not a
    // job of its own.
    let states = r.states(&job.job_id);
    let verifying = states
        .iter()
        .position(|s| *s == JobState::Verifying)
        .unwrap();
    assert_eq!(states.last(), Some(&JobState::Succeeded));
    assert!(verifying < states.len() - 1);

    // The check jobs never reached the UI: nothing but the two tool jobs
    // (leaky and clean) is in a snapshot. They aren't in the store at all.
    let snapshot_ids: HashSet<JobId> = r
        .app
        .get_snapshot()
        .jobs
        .into_iter()
        .map(|j| j.job_id)
        .collect();
    assert_eq!(snapshot_ids.len(), 2, "{snapshot_ids:?}");
    assert!(r.leftover_checks().is_empty(), "{:?}", r.leftover_checks());
}

/// `output-check` has a manifest like an operation's, but it is not one.
fn the_check_adapter_is_not_a_tool(r: &Rig) {
    let id = r.add("clean.pdf");
    let err = r
        .app
        .create_job(
            "output-check",
            std::slice::from_ref(&id),
            json!({"verifier": "activeContent"}),
        )
        .unwrap_err();
    assert_eq!(err.reason.as_deref(), Some("notAnOperation"), "{err:?}");

    let targets = r.app.get_targets(&[id]).unwrap();
    assert!(targets
        .operations
        .iter()
        .all(|o| o.adapter != "output-check"));
    assert!(targets.operations.iter().any(|o| o.adapter == "pdf-redact"));
    let json = serde_json::to_string(&targets).unwrap();
    assert!(!json.contains("output-check"));
}
