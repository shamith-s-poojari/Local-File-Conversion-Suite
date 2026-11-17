//! `lfcs` end to end on this machine. Needs the workspace virtualenv
//! (`uv sync`); without it, or on Linux without a working bubblewrap, each
//! test prints a note and passes. The binary is
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
    if !python.is_file() {
        eprintln!("skipped: no .venv; run `uv sync`");
        return None;
    }
    if !sandbox_works() {
        eprintln!("skipped: bubblewrap can't create a sandbox here");
        return None;
    }
    Some(python)
}

/// On Linux, bubblewrap may be blocked (a CI runner, a container); then
/// `lfcs` refuses to process (exit 3) and these tests skip, like the
/// sandbox crate's. On macOS and Windows the sandbox must work.
fn sandbox_works() -> bool {
    static WORKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WORKS.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return true;
        }
        let t = tempfile::tempdir().unwrap();
        let o = lfcs(&t.path().join("data"), &["doctor"]);
        o.status.code() != Some(3)
    })
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

/// `lfcs` with the repository as its runtime. A checkout is only used with
/// `--dev` (audit S1), so the tests ask for it.
fn lfcs(data: &Path, args: &[&str]) -> Output {
    lfcs_command(data, args).output().unwrap()
}

fn lfcs_command(data: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_lfcs"));
    c.env_remove("LFCS_DEV")
        .env_remove("LFCS_PASSWORD")
        .arg("--dev")
        .arg("--resources")
        .arg(repo())
        .arg("--data-dir")
        .arg(data)
        .args(args);
    c
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(o)).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

/// `--json` failures are `{ok: false, error: {code, reason, message}}`.
fn assert_error_shape(v: &serde_json::Value, code: &str) {
    assert_eq!(v["ok"], false, "{v}");
    let e = &v["error"];
    assert_eq!(e["code"], code, "{v}");
    for key in ["reason", "message"] {
        assert!(
            e[key].as_str().is_some_and(|s| !s.is_empty()),
            "error.{key}: {v}"
        );
    }
    assert_eq!(e.as_object().unwrap().len(), 3, "{v}");
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
    assert_eq!(v["ok"], true, "doctor --json says ok (audit U20)");
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

#[test]
fn a_checkout_needs_dev() {
    // Without --dev (or LFCS_DEV=1) a repository checkout is not a runtime,
    // so this needs no virtualenv (audit S1).
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("data");
    let plain = |env: &[(&str, &str)]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_lfcs"));
        c.env_remove("LFCS_DEV")
            .arg("--resources")
            .arg(repo())
            .arg("--data-dir")
            .arg(&data)
            .args(["doctor", "--json"]);
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    };
    let o = plain(&[]);
    assert_eq!(o.status.code(), Some(1), "{}{}", stdout(&o), stderr(&o));
    assert!(stderr(&o).contains("--dev"), "{}", stderr(&o));
    assert_error_shape(&json(&o), "E_INSTALL_CORRUPT");

    // LFCS_DEV=1 is the same switch (it is exactly "1", as in the core).
    if python().is_some() {
        let o = plain(&[("LFCS_DEV", "1")]);
        assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
        assert_eq!(json(&o)["ok"], true);
    }
    let o = plain(&[("LFCS_DEV", "yes")]);
    assert_eq!(o.status.code(), Some(1), "only 1 turns it on");
}

#[test]
fn usage_errors_have_the_json_shape() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("data");
    for (args, reason) in [
        (&["--json", "frobnicate"][..], "unknownCommand"),
        (&["--json"][..], "noCommand"),
        (&["--json", "--bogus"][..], "unknownOption"),
        (&["convert", "x.png", "--json"][..], "missingTarget"),
        (
            &["convert", "x.png", "--to", "nope", "--json"][..],
            "unknownFormat",
        ),
        (&["run", "pdf-merge", "--json"][..], "noFiles"),
        (&["doctor", "extra", "--json"][..], "unexpectedArguments"),
    ] {
        let o = lfcs(&d, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        let v = json(&o);
        assert_error_shape(&v, "E_USAGE");
        assert_eq!(v["error"]["reason"], reason, "{args:?}");
    }
}

#[test]
fn a_failed_job_has_the_json_shape() {
    let Some(_) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    // PNG signature, then nothing a decoder can use.
    let bad = t.path().join("broken.png");
    std::fs::write(&bad, [&b"\x89PNG\r\n\x1a\n"[..], &[7u8; 64][..]].concat()).unwrap();
    let o = lfcs(
        &t.path().join("data"),
        &[
            "convert",
            bad.to_str().unwrap(),
            "--to",
            "jpg",
            "--json",
            "-q",
        ],
    );
    assert_eq!(o.status.code(), Some(1), "{}{}", stdout(&o), stderr(&o));
    let v = json(&o);
    assert_error_shape(&v, "E_INPUT_CORRUPT");
    assert_eq!(v["state"], "failed");
    assert!(v["warnings"].is_array() && v["skipped"].is_array());
}

#[test]
fn inspect_json_reports_the_failure_in_the_same_shape() {
    let Some(_) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let bad = t.path().join("broken.png");
    std::fs::write(&bad, [&b"\x89PNG\r\n\x1a\n"[..], &[7u8; 64][..]].concat()).unwrap();
    let o = lfcs(
        &t.path().join("data"),
        &["inspect", bad.to_str().unwrap(), "--json"],
    );
    let v = json(&o);
    if o.status.code() == Some(1) {
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"].as_object().unwrap().len(), 3, "{v}");
        assert_eq!(v["files"][0]["error"], v["error"]);
    } else {
        // The inspector may accept a PNG this broken; then there is no error.
        assert_eq!(v["ok"], true, "{v}");
    }
}

#[test]
fn folder_truncation_is_reported() {
    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let folder = t.path().join("many");
    std::fs::create_dir_all(&folder).unwrap();
    let first = folder.join("0.png");
    py(
        &python,
        &format!(
            "from PIL import Image\nImage.new('RGB', (4, 4)).save('{}')\n",
            slashed(&first)
        ),
    );
    for i in 1..205 {
        std::fs::copy(&first, folder.join(format!("{i}.png"))).unwrap();
    }
    // `targets` only sniffs, so 200 files don't start 200 workers.
    let o = lfcs(
        &t.path().join("data"),
        &["targets", "--json", folder.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v = json(&o);
    let cut = v["folderTruncated"].as_array().unwrap();
    assert_eq!(cut.len(), 1, "{v}");
    assert_eq!(cut[0]["count"], 5);
    assert!(stderr(&o).contains("5 more files"), "{}", stderr(&o));
}

#[test]
fn passwords_come_from_stdin_or_the_environment() {
    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("a.pdf");
    make_pdf(&python, &a, 1);
    let data = t.path().join("data");

    // Never in arguments: refused, and not echoed.
    for args in [
        &["run", "pdf-protect", "x.pdf", "--password", "hunter2"][..],
        &[
            "run",
            "pdf-protect",
            "x.pdf",
            "--set",
            "userPassword=hunter2",
        ][..],
    ] {
        let o = lfcs(&data, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(stderr(&o).contains("--password-stdin"), "{}", stderr(&o));
        assert!(!stderr(&o).contains("hunter2") && !stdout(&o).contains("hunter2"));
    }

    // On standard input.
    let out = t.path().join("protected-stdin.pdf");
    let mut c = lfcs_command(
        &data,
        &[
            "run",
            "pdf-protect",
            a.to_str().unwrap(),
            "--password-stdin",
            "-q",
            "-o",
            out.to_str().unwrap(),
        ],
    );
    c.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"s3cret-pw\n")
            .unwrap();
    }
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    assert!(!stdout(&o).contains("s3cret-pw") && !stderr(&o).contains("s3cret-pw"));
    assert_opens_only_with(&python, &out, "s3cret-pw");

    // In the environment.
    let out = t.path().join("protected-env.pdf");
    let o = lfcs_command(
        &data,
        &[
            "run",
            "pdf-protect",
            a.to_str().unwrap(),
            "-q",
            "-o",
            out.to_str().unwrap(),
        ],
    )
    .env("LFCS_PASSWORD", "env-pw-1")
    .output()
    .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    assert_opens_only_with(&python, &out, "env-pw-1");

    // Asked for and not given.
    let o = lfcs_command(
        &data,
        &[
            "run",
            "pdf-protect",
            a.to_str().unwrap(),
            "--password-stdin",
            "-q",
        ],
    )
    .stdin(std::process::Stdio::null())
    .output()
    .unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
}

/// The PDF is encrypted: it opens with `password` and not without one.
fn assert_opens_only_with(python: &Path, pdf: &Path, password: &str) {
    py(
        python,
        &format!(
            "import pikepdf\n\
             try:\n\
             \x20   pikepdf.open('{p}')\n\
             \x20   raise SystemExit('opened without a password')\n\
             except pikepdf.PasswordError:\n\
             \x20   pass\n\
             pikepdf.open('{p}', password='{pw}')\n",
            p = slashed(pdf),
            pw = password
        ),
    );
}

/// Ctrl-C stops a running job, removes its folder and exits 130 (audit R17).
#[cfg(unix)]
#[test]
fn ctrl_c_cancels_and_cleans_up() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let Some(python) = python() else { return };
    let t = tempfile::tempdir().unwrap();
    // A big picture of noise: slow to encode, so the job is still running
    // when the signal arrives.
    let png = t.path().join("noise.png");
    py(
        &python,
        &format!(
            "import os\nfrom PIL import Image\n\
             Image.frombytes('RGB', (6000, 6000), os.urandom(6000 * 6000 * 3)).save('{}', compress_level=0)\n",
            slashed(&png)
        ),
    );
    let data = t.path().join("data");
    let out = t.path().join("out");
    let mut child = lfcs_command(
        &data,
        &[
            "convert",
            png.to_str().unwrap(),
            "--to",
            "webp",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    // Progress lines on stderr tell when the job is running.
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = Vec::new();
    for line in lines.by_ref() {
        let line = line.unwrap();
        let running = line.starts_with("[running");
        seen.push(line);
        if running {
            break;
        }
    }
    assert!(
        seen.iter().any(|l| l.starts_with("[running")),
        "the job never ran: {seen:?}"
    );
    let signalled = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signalled.success());
    let rest: Vec<String> = lines.map(Result::unwrap).collect();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "{seen:?} {rest:?}");
    assert!(rest.iter().any(|l| l.contains("Cancelled")), "{rest:?}");
    assert!(!out.exists() || std::fs::read_dir(&out).unwrap().next().is_none());
    // The session's shutdown removed the job folder.
    let jobs = data.join("jobs");
    let left: Vec<_> = std::fs::read_dir(&jobs)
        .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "job folders left behind: {left:?}");
}
