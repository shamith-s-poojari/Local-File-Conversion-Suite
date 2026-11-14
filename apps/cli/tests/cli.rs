//! `lfcs` end to end on this machine. Needs the workspace virtualenv
//! (`uv sync`); without it each test prints a note and passes. The binary is
//! its own sandbox runner, so these run real sandboxed workers.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn python() -> Option<PathBuf> {
    let python = if cfg!(windows) {
        repo().join(".venv/Scripts/python.exe")
    } else {
        repo().join(".venv/bin/python")
    };
    if python.is_file() {
        Some(python)
    } else {
        eprintln!("skipped: no .venv; run `uv sync`");
        None
    }
}

/// Forward slashes: safe inside Python source on every OS.
fn slashed(p: &Path) -> String {
    p.to_str().unwrap().replace('\\', "/")
}

fn py(python: &Path, code: &str) {
    let ok = Command::new(python)
        .args(["-I", "-c", code])
        .status()
        .unwrap()
        .success();
    assert!(ok, "python helper failed");
}

fn lfcs(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lfcs"))
        .arg("--resources")
        .arg(repo())
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn make_pdf(python: &Path, path: &Path, pages: u32) {
    py(
        python,
        &format!(
            "import pikepdf\n\
             pdf = pikepdf.new()\n\
             font = pdf.make_indirect(pikepdf.Dictionary(Type=pikepdf.Name.Font, Subtype=pikepdf.Name.Type1, BaseFont=pikepdf.Name.Helvetica))\n\
             for n in range(1, {pages} + 1):\n\
             \x20   c = pdf.make_stream(('BT /F1 24 Tf 72 700 Td (Page %d) Tj ET' % n).encode())\n\
             \x20   pdf.pages.append(pikepdf.Page(pdf.make_indirect(pikepdf.Dictionary(Type=pikepdf.Name.Page, MediaBox=[0,0,612,792], Resources=pikepdf.Dictionary(Font=pikepdf.Dictionary(F1=font)), Contents=c))))\n\
             pdf.save('{}')\n",
            slashed(path)
        ),
    );
}

#[test]
fn doctor_passes() {
    let Some(_) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let o = lfcs(&t.path().join("data"), &["doctor"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    assert!(stdout(&o).contains("sandbox: passed"), "{}", stdout(&o));
}

#[test]
fn doctor_json() {
    let Some(_) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let o = lfcs(&t.path().join("data"), &["doctor", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["processingEnabled"], true);
}

#[test]
fn convert_png_to_jpg() {
    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let png = t.path().join("photo.png");
    py(
        &python,
        &format!(
            "from PIL import Image\nImage.new('RGB', (40, 30), (200, 30, 30)).save('{}')\n",
            slashed(&png)
        ),
    );
    let out = t.path().join("out");
    let o = lfcs(
        &t.path().join("data"),
        &[
            "convert",
            png.to_str().unwrap(),
            "--to",
            "jpg",
            "--json",
            "-q",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["ok"], true);
    let saved = out.join("photo.jpg");
    assert!(saved.is_file(), "{v}");
    assert_eq!(v["saved"].as_array().unwrap().len(), 1);
    let head = std::fs::read(&saved).unwrap();
    assert_eq!(&head[..2], &[0xFF, 0xD8], "a JPEG");

    // Nothing is overwritten: a second run gets a suffixed name.
    let again = lfcs(
        &t.path().join("data"),
        &[
            "convert",
            png.to_str().unwrap(),
            "--to",
            "jpg",
            "-q",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2);
}

#[test]
fn run_pdf_merge() {
    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let (a, b) = (t.path().join("a.pdf"), t.path().join("b.pdf"));
    make_pdf(&python, &a, 2);
    make_pdf(&python, &b, 3);
    let merged = t.path().join("merged.pdf");
    let o = lfcs(
        &t.path().join("data"),
        &[
            "run",
            "pdf-merge",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            "-q",
            "--out",
            merged.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    assert!(merged.is_file());
    py(
        &python,
        &format!(
            "import pikepdf\nassert len(pikepdf.open('{}').pages) == 5\n",
            slashed(&merged)
        ),
    );

    // An existing single-file --out needs --force.
    let refused = lfcs(
        &t.path().join("data"),
        &[
            "run",
            "pdf-merge",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            "-q",
            "--out",
            merged.to_str().unwrap(),
        ],
    );
    assert_eq!(refused.status.code(), Some(2), "{}", stderr(&refused));
    let forced = lfcs(
        &t.path().join("data"),
        &[
            "run",
            "pdf-merge",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            "-q",
            "--force",
            "--out",
            merged.to_str().unwrap(),
        ],
    );
    assert_eq!(forced.status.code(), Some(0), "{}", stderr(&forced));
}

#[test]
fn inspect_and_targets() {
    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("a.pdf");
    make_pdf(&python, &a, 2);
    let o = lfcs(&t.path().join("data"), &["inspect", a.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).contains("format: pdf"), "{}", stdout(&o));
    assert!(stdout(&o).contains("pages: 2"), "{}", stdout(&o));
    let o = lfcs(
        &t.path().join("data"),
        &["targets", "--json", a.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(v["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|op| op["adapter"] == "pdf-merge"));
}

#[test]
fn usage_errors_exit_2() {
    // These never need the runtime.
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("data");
    for args in [
        &[][..],
        &["frobnicate"][..],
        &["convert", "x.png"][..],
        &["convert", "--to", "jpg"][..],
        &["run", "pdf-merge"][..],
        &["doctor", "--bogus"][..],
        &["convert", "x.png", "--to", "jpg", "--set", "novalue"][..],
        &["convert", "x.png", "--to", "jpg", "--params", "[1]"][..],
    ] {
        let o = lfcs(&d, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
    }
    let o = lfcs(&d, &["--version"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).starts_with("lfcs "));
    let o = lfcs(&d, &["--help"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("USAGE"));
}

#[test]
fn unknown_tool_is_a_usage_error() {
    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("a.pdf");
    make_pdf(&python, &a, 1);
    let o = lfcs(
        &t.path().join("data"),
        &["run", "no-such-tool", a.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
}
