//! Saving into a folder with the dropped folder's layout kept.

use std::path::{Path, PathBuf};

use converter_jobstore::{
    DataRoot, FolderLayout, InputSource, JobId, JobState, JobStore, OutputFile, SaveDestination,
    SaveOptions, StoreConfig, SyncRoots,
};

struct Fixture {
    tmp: tempfile::TempDir,
    store: JobStore,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
    let (store, _) = JobStore::open(root, SyncRoots::none(), StoreConfig::default()).unwrap();
    Fixture { tmp, store }
}

impl Fixture {
    /// A finished job with one input per output; `outputs` are (name, source).
    fn finish(&self, outputs: &[(&str, Option<usize>)]) -> JobId {
        let user = self.tmp.path().join("user");
        std::fs::create_dir_all(&user).unwrap();
        let sources: Vec<InputSource> = (0..2)
            .map(|i| {
                let p = user.join(format!("in{i}.pdf"));
                std::fs::write(&p, b"%PDF").unwrap();
                InputSource {
                    path: p,
                    extension: "pdf".into(),
                }
            })
            .collect();
        let (id, _) = self.store.create_job().unwrap();
        self.store.stage_inputs(&id, &sources, 2.0).unwrap();
        for s in [JobState::Validating, JobState::Queued, JobState::Running] {
            self.store.transition(&id, s).unwrap();
        }
        let dirs = self.store.job_dirs(&id).unwrap();
        let mut files = Vec::new();
        for (n, (name, source)) in outputs.iter().enumerate() {
            let path = format!("{n}.pdf");
            std::fs::write(dirs.out.join(&path), name.as_bytes()).unwrap();
            files.push(OutputFile {
                path,
                suggested_name: (*name).into(),
                extension: "pdf".into(),
                size_bytes: name.len() as u64,
                source: *source,
            });
        }
        self.store.transition(&id, JobState::Verifying).unwrap();
        self.store.record_outputs(&id, files).unwrap();
        self.store.transition(&id, JobState::Succeeded).unwrap();
        id
    }

    fn dest(&self) -> PathBuf {
        let d = self.tmp.path().join("dest");
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

fn layout(dir: &Path, inputs: &[Option<&str>]) -> SaveDestination {
    SaveDestination::FolderKeepingLayout(FolderLayout {
        dir: dir.to_path_buf(),
        inputs: inputs.iter().map(|i| i.map(str::to_owned)).collect(),
    })
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn keeps_the_folder_layout_and_never_overwrites() {
    let f = fixture();
    let id = f.finish(&[("a.pdf", Some(0)), ("a.pdf", Some(0)), ("b.pdf", Some(1))]);
    let dest = f.dest();
    std::fs::create_dir_all(dest.join("photos/2024")).unwrap();
    std::fs::write(dest.join("photos/2024/a.pdf"), b"mine").unwrap();
    let report = f
        .store
        .save_outputs(
            &id,
            &layout(&dest, &[Some("photos/2024"), Some("photos")]),
            SaveOptions::default(),
        )
        .unwrap();
    let got: Vec<String> = report.saved.iter().map(|p| rel(&dest, p)).collect();
    assert_eq!(
        got,
        [
            "photos/2024/a (1).pdf",
            "photos/2024/a (2).pdf",
            "photos/b.pdf"
        ]
    );
    assert_eq!(
        std::fs::read(dest.join("photos/2024/a.pdf")).unwrap(),
        b"mine"
    );
}

#[test]
fn outputs_without_a_folder_go_in_the_chosen_folder() {
    let f = fixture();
    let id = f.finish(&[("a.pdf", None), ("b.pdf", Some(1)), ("c.pdf", Some(9))]);
    let dest = f.dest();
    let report = f
        .store
        .save_outputs(
            &id,
            &layout(&dest, &[None, Some("root")]),
            SaveOptions::default(),
        )
        .unwrap();
    let got: Vec<String> = report.saved.iter().map(|p| rel(&dest, p)).collect();
    assert_eq!(got, ["a.pdf", "root/b.pdf", "c.pdf"]);
}

#[test]
fn components_cannot_escape_the_destination() {
    let f = fixture();
    let id = f.finish(&[("a.pdf", Some(0)), ("b.pdf", Some(1))]);
    let dest = f.dest();
    let report = f
        .store
        .save_outputs(
            &id,
            &layout(
                &dest,
                &[Some("../../evil/./C:\\Windows//x"), Some("/etc/..")],
            ),
            SaveOptions::default(),
        )
        .unwrap();
    for p in &report.saved {
        assert!(p.starts_with(&dest), "{p:?} left the destination");
        assert!(!rel(&dest, p).contains(".."));
    }
    let got: Vec<String> = report.saved.iter().map(|p| rel(&dest, p)).collect();
    assert_eq!(got[0], "evil/C/Windows/x/a.pdf");
    assert!(!f.tmp.path().join("evil").exists());
}

#[cfg(unix)]
#[test]
fn a_link_in_the_destination_is_not_followed() {
    let f = fixture();
    let id = f.finish(&[("a.pdf", Some(0))]);
    let dest = f.dest();
    let elsewhere = f.tmp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dest.join("root")).unwrap();
    let report = f
        .store
        .save_outputs(&id, &layout(&dest, &[Some("root")]), SaveOptions::default())
        .unwrap();
    assert_eq!(rel(&dest, &report.saved[0]), "root (1)/a.pdf");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn a_failed_save_removes_what_it_made() {
    let f = fixture();
    let id = f.finish(&[("a.pdf", Some(0)), ("b.pdf", Some(0))]);
    // Break the second output after it was verified.
    let dirs = f.store.job_dirs(&id).unwrap();
    std::fs::write(dirs.out.join("1.pdf"), b"changed after verification!").unwrap();
    let dest = f.dest();
    let err = f
        .store
        .save_outputs(
            &id,
            &layout(&dest, &[Some("root/sub")]),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert_eq!(err.code(), Some("E_PROTOCOL"));
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "{:?}", dest);
}
