//! Job store behavior: lifecycle, staging, cleanup, sweeping and saving.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use converter_jobstore::{
    DataRoot, InputSource, JobError, JobEvent, JobFailure, JobId, JobState, JobStore, Lease,
    OutputFile, Progress, RetryPolicy, SaveDestination, SaveOptions, StoreConfig, SyncOs,
    SyncProvider, SyncRoots, Warning,
};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    store: JobStore,
    events: Arc<Mutex<Vec<JobEvent>>>,
}

fn config(events: &Arc<Mutex<Vec<JobEvent>>>) -> StoreConfig {
    let sink = events.clone();
    StoreConfig {
        retry: RetryPolicy {
            retries: 2,
            base_delay: Duration::from_millis(1),
        },
        orphan_grace: Duration::from_secs(60),
        on_event: Some(Arc::new(move |e: &JobEvent| {
            sink.lock().unwrap().push(e.clone())
        })),
    }
}

fn fixture_with(sync: SyncRoots) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let (store, report) = JobStore::open(root, sync, config(&events)).unwrap();
    assert_eq!(report.removed + report.kept + report.failed, 0);
    Fixture { tmp, store, events }
}

fn fixture() -> Fixture {
    fixture_with(SyncRoots::none())
}

impl Fixture {
    fn user_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = self.tmp.path().join("user");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn jobs_dir(&self) -> &Path {
        self.store.data_root().jobs_dir()
    }

    /// Runs a job up to `verifying`, with the given outputs written into out/.
    fn run_to_verifying(&self, outputs: &[(&str, &str, &[u8])]) -> (JobId, PathBuf) {
        let input = self.user_file("input.pdf", b"%PDF-1.7 input");
        let (id, _) = self.store.create_job().unwrap();
        self.store
            .stage_inputs(
                &id,
                &[InputSource {
                    path: input.clone(),
                    extension: "pdf".into(),
                }],
                2.0,
            )
            .unwrap();
        for s in [JobState::Validating, JobState::Queued, JobState::Running] {
            self.store.transition(&id, s).unwrap();
        }
        let dirs = self.store.job_dirs(&id).unwrap();
        for (path, _, bytes) in outputs {
            std::fs::write(dirs.out.join(path), bytes).unwrap();
        }
        self.store.transition(&id, JobState::Verifying).unwrap();
        (id, input)
    }

    fn finish(&self, outputs: &[(&str, &str, &[u8])]) -> (JobId, PathBuf) {
        let (id, input) = self.run_to_verifying(outputs);
        self.store
            .record_outputs(
                &id,
                outputs
                    .iter()
                    .map(|(path, name, bytes)| OutputFile {
                        path: (*path).into(),
                        suggested_name: (*name).into(),
                        extension: "pdf".into(),
                        size_bytes: bytes.len() as u64,
                        source: None,
                    })
                    .collect(),
            )
            .unwrap();
        self.store.transition(&id, JobState::Succeeded).unwrap();
        (id, input)
    }
}

#[test]
fn happy_path_events_are_sequential_and_cleanup_follows_the_table() {
    let f = fixture();
    let (id, _) = f.run_to_verifying(&[("0.pdf", "merged.pdf", b"%PDF out")]);
    let dirs = f.store.job_dirs(&id).unwrap();
    assert_eq!(dirs.inputs, vec![dirs.input_dir.join("0.pdf")]);
    assert_eq!(std::fs::read(&dirs.inputs[0]).unwrap(), b"%PDF-1.7 input");
    assert!(std::fs::metadata(&dirs.inputs[0])
        .unwrap()
        .permissions()
        .readonly());
    assert!(dirs.root.join(".lease").is_file());

    f.store.set_stage(&id, "verify").unwrap();
    f.store
        .set_progress(
            &id,
            Progress {
                done: 1,
                total: Some(2),
            },
        )
        .unwrap();
    let w = f
        .store
        .add_warning(
            &id,
            Warning {
                code: "TAGS_DROPPED".into(),
                detail: None,
            },
        )
        .unwrap();
    assert_eq!(w.warnings.len(), 1);
    f.store
        .record_outputs(
            &id,
            vec![OutputFile {
                path: "0.pdf".into(),
                suggested_name: "merged.pdf".into(),
                extension: "pdf".into(),
                size_bytes: 8,
                source: None,
            }],
        )
        .unwrap();
    std::fs::write(dirs.work.join("scratch"), b"x").unwrap();
    f.store.transition(&id, JobState::Succeeded).unwrap();

    // Terminal success deletes work/ only.
    assert!(!dirs.work.exists());
    assert!(dirs.out.join("0.pdf").is_file());
    assert!(dirs.inputs[0].is_file());

    let events = f.events.lock().unwrap();
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    let states: Vec<JobState> = events.iter().map(|e| e.state).collect();
    assert_eq!(states.first(), Some(&JobState::Created));
    assert_eq!(states.last(), Some(&JobState::Succeeded));
    assert_eq!(events[1].stage.as_deref(), Some("preparing"));

    let snap = f.store.snapshot();
    assert_eq!(snap.seq, *seqs.last().unwrap());
    assert_eq!(snap.jobs.len(), 1);
    assert_eq!(snap.jobs[0].warnings.len(), 1);
    assert_eq!(snap.jobs[0].outputs.len(), 1);
    // The snapshot never carries paths.
    let json = serde_json::to_string(&snap).unwrap();
    assert!(!json.contains("input.pdf") && !json.contains(&*f.tmp.path().to_string_lossy()));
}

#[test]
fn invalid_transitions_are_refused() {
    let f = fixture();
    let (id, _) = f.store.create_job().unwrap();
    let err = f.store.transition(&id, JobState::Running).unwrap_err();
    assert!(matches!(
        err,
        JobError::InvalidTransition {
            from: JobState::Created,
            to: JobState::Running
        }
    ));
    f.store.transition(&id, JobState::Cancelled).unwrap();
    assert!(f.store.transition(&id, JobState::Failed).is_err());
    assert!(f.store.set_stage(&id, "x").is_err());
    let unknown = JobId::parse("j_nope").unwrap();
    assert!(matches!(
        f.store.transition(&unknown, JobState::Staging),
        Err(JobError::UnknownJob(_))
    ));
}

#[test]
fn failure_deletes_in_work_and_out() {
    let f = fixture();
    let (id, _) = f.run_to_verifying(&[("0.pdf", "x.pdf", b"out")]);
    let dirs = f.store.job_dirs(&id).unwrap();
    let ev = f
        .store
        .fail(
            &id,
            JobFailure {
                code: "E_OUTPUT_INVALID".into(),
                retryable: false,
            },
        )
        .unwrap();
    assert_eq!(ev.error.unwrap().code, "E_OUTPUT_INVALID");
    for d in [&dirs.input_dir, &dirs.work, &dirs.out] {
        assert!(!d.exists());
    }
    // Discarding removes the rest.
    f.store.discard(&id).unwrap();
    assert!(!dirs.root.exists());
    assert!(f.store.snapshot().jobs.is_empty());
}

#[test]
fn memory_retry_resets_work_and_out() {
    let f = fixture();
    let input = f.user_file("r.pdf", b"%PDF");
    let (id, _) = f.store.create_job().unwrap();
    f.store
        .stage_inputs(
            &id,
            &[InputSource {
                path: input,
                extension: "pdf".into(),
            }],
            0.0,
        )
        .unwrap();
    for s in [JobState::Validating, JobState::Queued, JobState::Running] {
        f.store.transition(&id, s).unwrap();
    }
    let dirs = f.store.job_dirs(&id).unwrap();
    std::fs::write(dirs.out.join("half.pdf"), b"half").unwrap();
    f.store.transition(&id, JobState::Queued).unwrap();
    assert!(dirs.out.is_dir() && dirs.work.is_dir());
    assert_eq!(std::fs::read_dir(&dirs.out).unwrap().count(), 0);
    assert!(dirs.inputs[0].is_file());
}

#[test]
fn staging_rejects_bad_requests_and_impossible_disk() {
    let f = fixture();
    let (id, _) = f.store.create_job().unwrap();
    let input = f.user_file("a.pdf", b"%PDF");
    let src = |ext: &str| InputSource {
        path: input.clone(),
        extension: ext.into(),
    };
    assert!(matches!(
        f.store.stage_inputs(&id, &[], 1.0),
        Err(JobError::InvalidInput(_))
    ));
    assert!(matches!(
        f.store.stage_inputs(&id, &[src("../pdf")], 1.0),
        Err(JobError::InvalidInput(_))
    ));
    assert!(matches!(
        f.store.stage_inputs(&id, &[src("pdf")], f64::NAN),
        Err(JobError::InvalidInput(_))
    ));
    // A disk factor no disk can satisfy.
    let err = f.store.stage_inputs(&id, &[src("pdf")], 1e30).unwrap_err();
    assert!(matches!(err, JobError::InsufficientDisk { .. }));
    assert_eq!(err.code(), Some("E_RESOURCE_DISK"));
    f.store
        .fail(
            &id,
            JobFailure {
                code: err.code().unwrap().into(),
                retryable: true,
            },
        )
        .unwrap();
    assert!(!f.store.job_dirs(&id).unwrap().input_dir.exists());
}

#[test]
fn staging_a_directory_is_refused() {
    let f = fixture();
    let (id, _) = f.store.create_job().unwrap();
    let err = f
        .store
        .stage_inputs(
            &id,
            &[InputSource {
                path: f.tmp.path().to_path_buf(),
                extension: "pdf".into(),
            }],
            1.0,
        )
        .unwrap_err();
    assert!(matches!(err, JobError::InvalidInput(_)));
}

#[test]
fn record_outputs_distrusts_the_worker() {
    let f = fixture();
    let (id, _) = f.run_to_verifying(&[("0.pdf", "x.pdf", b"12345")]);
    let out = |path: &str, size: u64| OutputFile {
        path: path.into(),
        suggested_name: "x.pdf".into(),
        extension: "pdf".into(),
        size_bytes: size,
        source: None,
    };
    for bad in [
        out("../escape.pdf", 1),
        out("/abs.pdf", 1),
        out("a\\b.pdf", 1),
        out("", 1),
        out("missing.pdf", 1),
        out("0.pdf", 4),
    ] {
        let err = f.store.record_outputs(&id, vec![bad.clone()]).unwrap_err();
        assert_eq!(err.code(), Some("E_PROTOCOL"), "{bad:?}");
    }
    #[cfg(unix)]
    {
        let dirs = f.store.job_dirs(&id).unwrap();
        let secret = f.user_file("secret.txt", b"12345");
        std::os::unix::fs::symlink(&secret, dirs.out.join("link.pdf")).unwrap();
        let err = f
            .store
            .record_outputs(&id, vec![out("link.pdf", 5)])
            .unwrap_err();
        assert_eq!(err.code(), Some("E_PROTOCOL"));
    }
    f.store.record_outputs(&id, vec![out("0.pdf", 5)]).unwrap();
}

#[test]
fn save_single_file_atomically_then_delete_job() {
    let f = fixture();
    let (id, _) = f.finish(&[("0.pdf", "merged.pdf", b"%PDF merged")]);
    let root = f.store.job_dirs(&id).unwrap().root;
    let dest_dir = f.tmp.path().join("saves");
    std::fs::create_dir(&dest_dir).unwrap();
    let dest = dest_dir.join("chosen.pdf");
    std::fs::write(&dest, b"old").unwrap(); // the dialog confirmed replacing it

    let report = f
        .store
        .save_outputs(
            &id,
            &SaveDestination::File(dest.clone()),
            SaveOptions::default(),
        )
        .unwrap();
    assert_eq!(report.saved, vec![dest.clone()]);
    assert!(!report.cleanup_deferred);
    assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF merged");
    // No temp files left behind, and the job is gone.
    assert_eq!(std::fs::read_dir(&dest_dir).unwrap().count(), 1);
    assert!(!root.exists());
    assert!(f.store.snapshot().jobs.is_empty());
    // A saved result gets ordinary permissions, not the job's private 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_ne!(
            mode, 0o600,
            "saved files must be readable like any new file"
        );
        assert_eq!(mode & 0o600, 0o600);
    }
}

#[test]
fn folder_save_never_overwrites_and_sanitizes_names() {
    let f = fixture();
    let (id, _) = f.finish(&[
        ("0.pdf", "../../part.pdf", b"one"),
        ("1.pdf", "part.pdf", b"two"),
        ("2.pdf", "", b"three"),
    ]);
    let folder = f.tmp.path().join("out-folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("part.pdf"), b"user's own").unwrap();

    let report = f
        .store
        .save_outputs(
            &id,
            &SaveDestination::Folder(folder.clone()),
            SaveOptions::default(),
        )
        .unwrap();
    let names: Vec<String> = report
        .saved
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["part (1).pdf", "part (2).pdf", "output-3.pdf"]);
    assert_eq!(
        std::fs::read(folder.join("part.pdf")).unwrap(),
        b"user's own"
    );
    assert_eq!(std::fs::read(folder.join("part (2).pdf")).unwrap(), b"two");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 4);
}

#[test]
fn several_outputs_need_a_folder() {
    let f = fixture();
    let (id, _) = f.finish(&[("0.pdf", "a.pdf", b"a"), ("1.pdf", "b.pdf", b"b")]);
    let err = f
        .store
        .save_outputs(
            &id,
            &SaveDestination::File(f.tmp.path().join("x.pdf")),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert!(matches!(err, JobError::InvalidInput(_)));
}

#[test]
fn saving_over_an_input_needs_confirmation() {
    let f = fixture();
    let (id, input) = f.finish(&[("0.pdf", "a.pdf", b"new content")]);
    let dest = SaveDestination::File(input.clone());
    assert!(f.store.check_save(&id, &dest).unwrap().overwrites_input);
    let err = f
        .store
        .save_outputs(&id, &dest, SaveOptions::default())
        .unwrap_err();
    assert!(matches!(err, JobError::NeedsInputOverwriteConfirmation));
    assert_eq!(std::fs::read(&input).unwrap(), b"%PDF-1.7 input");

    // A different spelling of the same path is caught too.
    let dotted = input.parent().unwrap().join(".").join("input.pdf");
    assert!(
        f.store
            .check_save(&id, &SaveDestination::File(dotted))
            .unwrap()
            .overwrites_input
    );

    f.store
        .save_outputs(
            &id,
            &dest,
            SaveOptions {
                confirmed_input_overwrite: true,
            },
        )
        .unwrap();
    assert_eq!(std::fs::read(&input).unwrap(), b"new content");
}

#[test]
fn sync_folder_destination_is_a_notice_only() {
    let tmp_sync = tempfile::tempdir().unwrap();
    let sync = SyncRoots::from_roots(
        vec![(tmp_sync.path().to_path_buf(), SyncProvider::Dropbox)],
        SyncOs::current(),
    );
    let f = fixture_with(sync);
    let (id, _) = f.finish(&[("0.pdf", "a.pdf", b"a")]);
    let dest = SaveDestination::Folder(tmp_sync.path().to_path_buf());
    assert_eq!(
        f.store.check_save(&id, &dest).unwrap().sync_provider,
        Some(SyncProvider::Dropbox)
    );
    let report = f
        .store
        .save_outputs(&id, &dest, SaveOptions::default())
        .unwrap();
    assert_eq!(report.sync_provider, Some(SyncProvider::Dropbox));
    assert!(tmp_sync.path().join("a.pdf").is_file());
}

#[test]
fn failed_save_keeps_the_job_for_another_try() {
    let f = fixture();
    let (id, _) = f.finish(&[("0.pdf", "a.pdf", b"a")]);
    let missing = f.tmp.path().join("no-such-folder");
    assert!(f
        .store
        .save_outputs(
            &id,
            &SaveDestination::Folder(missing),
            SaveOptions::default()
        )
        .is_err());
    assert_eq!(f.store.snapshot().jobs.len(), 1);
    f.store
        .save_outputs(
            &id,
            &SaveDestination::File(f.tmp.path().join("ok.pdf")),
            SaveOptions::default(),
        )
        .unwrap();
}

#[test]
fn unfinished_jobs_cannot_be_saved_or_discarded() {
    let f = fixture();
    let (id, _) = f.run_to_verifying(&[]);
    assert!(matches!(
        f.store.discard(&id),
        Err(JobError::WrongState { .. })
    ));
    assert!(matches!(
        f.store.save_outputs(
            &id,
            &SaveDestination::Folder(f.tmp.path().to_path_buf()),
            SaveOptions::default()
        ),
        Err(JobError::WrongState { .. })
    ));
}

#[test]
fn shutdown_deletes_every_job_directory() {
    let f = fixture();
    let (a, _) = f.store.create_job().unwrap();
    let _ = f.finish(&[("0.pdf", "x.pdf", b"x")]);
    let a_dir = f.store.job_dirs(&a).unwrap().root;
    let report = f.store.shutdown();
    assert_eq!(report.removed, 2);
    assert_eq!(report.failed, 0);
    assert!(!a_dir.exists());
    assert_eq!(std::fs::read_dir(f.jobs_dir()).unwrap().count(), 0);
}

#[test]
fn startup_sweep_removes_dead_owners_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let jobs = root.jobs_dir().to_path_buf();

    let write_lease = |dir: &Path, lease: Lease| {
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out").join("o.pdf"), b"x").unwrap();
        std::fs::write(dir.join(".lease"), serde_json::to_vec(&lease).unwrap()).unwrap();
    };
    let me = Lease::current().unwrap();
    // A live owner (this process stands in for another running instance).
    write_lease(&jobs.join("j_live"), me);
    // Same PID, different start time: the PID was reused.
    write_lease(
        &jobs.join("j_reused"),
        Lease {
            pid: me.pid,
            start_time: me.start_time - 10_000,
        },
    );
    // No lease, but brand new: maybe another instance is creating it.
    std::fs::create_dir(jobs.join("j_new")).unwrap();
    // A stray file.
    std::fs::write(jobs.join("stray"), b"x").unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let (_store, report) =
        JobStore::open(root.clone(), SyncRoots::none(), config(&events)).unwrap();
    assert_eq!(report.removed, 2);
    assert_eq!(report.kept, 2);
    assert!(jobs.join("j_live").exists());
    assert!(jobs.join("j_new").exists());
    assert!(!jobs.join("j_reused").exists());
    assert!(!jobs.join("stray").exists());

    // With no grace period, the lease-less directory is collected too.
    let mut cfg = config(&events);
    cfg.orphan_grace = Duration::ZERO;
    let (_store, report) = JobStore::open(root, SyncRoots::none(), cfg).unwrap();
    assert_eq!(report.removed, 1);
    assert!(!jobs.join("j_new").exists());
}

#[test]
fn seq_is_shared_across_jobs() {
    let f = fixture();
    let (a, e1) = f.store.create_job().unwrap();
    let (b, e2) = f.store.create_job().unwrap();
    let e3 = f.store.transition(&a, JobState::Cancelled).unwrap();
    let e4 = f.store.transition(&b, JobState::Cancelled).unwrap();
    assert_eq!(
        [e1.seq, e2.seq, e3.seq, e4.seq],
        [1, 2, 3, 4],
        "one counter for the whole app"
    );
}
