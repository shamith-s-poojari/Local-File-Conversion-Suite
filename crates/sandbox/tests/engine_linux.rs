//! FFmpeg's tools run inside the Linux sandbox. The end-to-end media steps
//! fail there with only `E_INPUT_CORRUPT`, because the worker never shows
//! engine output (it can contain file text). This runs `ffprobe` on a
//! generated tone, so a failure prints what ffprobe said.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use converter_sandbox::{SandboxConfig, SandboxLauncher};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn skip(why: &str) {
    assert!(
        std::env::var_os("CI").is_none(),
        "not skipping on CI: {why}"
    );
    eprintln!("skipped: {why}");
}

/// A one-second mono 16-bit WAV of a sine tone.
fn tone(path: &Path) {
    let rate: u32 = 44_100;
    let samples: Vec<u8> = (0..rate)
        .flat_map(|n| ((9000.0 * (f64::from(n) / 10.0).sin()) as i16).to_le_bytes())
        .collect();
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    w.extend_from_slice(&samples);
    std::fs::write(path, w).unwrap();
}

#[test]
fn ffprobe_reads_a_tone_inside_the_sandbox() {
    let python = repo_root().join(".venv/bin/python");
    if !python.is_file() {
        return skip("no .venv; run `uv sync`");
    }
    let ffprobe = Path::new("/usr/bin/ffprobe");
    if !ffprobe.is_file() {
        return skip("FFmpeg not installed in /usr/bin");
    }
    let config = SandboxConfig::discover(
        &python,
        &repo_root().join("adapters"),
        Path::new(env!("CARGO_BIN_EXE_converter-sandbox-run")),
    )
    .expect("runtime discovery");
    let launcher = SandboxLauncher::new(config);
    let tmp = tempfile::tempdir().unwrap();
    let job = tmp.path().join("j_ffprobe");
    std::fs::create_dir_all(job.join("work")).unwrap();
    std::fs::create_dir_all(job.join("in")).unwrap();
    let wav = job.join("in/0.wav");
    tone(&wav);
    let argv = vec![
        ffprobe.as_os_str().to_owned(),
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_streams".into(),
        std::fs::canonicalize(&wav).unwrap().into_os_string(),
    ];
    let out = launcher
        .wrap("j_ffprobe", &job, 1 << 30, argv)
        .expect("wrap")
        .stdin(Stdio::null())
        .output()
        .expect("start the runner");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("\"codec_type\": \"audio\""),
        "ffprobe in the sandbox: {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
}

/// What the fake adapter below runs: LibreOffice as `converter_sdk.office`
/// starts it, printing its exit code, stderr and what it wrote.
const OFFICE_SCRIPT: &str = r#"
import os, pathlib, subprocess, sys
work = pathlib.Path("work").resolve()
src = work / "note.html"
src.write_text("<p>Hello</p>")
out = work / "lo-out"
out.mkdir()
env = {"HOME": str(work), "TMPDIR": str(work), "TEMP": str(work), "TMP": str(work)}
p = subprocess.run(
    [os.environ["LFCS_BIN_SOFFICE"], "-env:UserInstallation=" + (work / "lo-profile").as_uri(),
     "--headless", "--invisible", "--nologo", "--norestore", "--nodefault",
     "--nolockcheck", "--nofirststartwizard",
     "--convert-to", "pdf", "--outdir", str(out), str(src)],
    stdin=subprocess.DEVNULL, capture_output=True, env=env, cwd=work, timeout=240,
)
print("EXIT", p.returncode)
print("STDOUT", p.stdout.decode(errors="replace")[-4000:])
print("STDERR", p.stderr.decode(errors="replace")[-4000:])
print("PRODUCED", sorted(x.name for x in out.iterdir()))
"#;

#[test]
fn libreoffice_converts_inside_the_sandbox() {
    use converter_jobstore::JobId;
    use converter_sandbox::Grant;
    use converter_scheduler::{LaunchSpec, Launcher};

    let python = repo_root().join(".venv/bin/python");
    if !python.is_file() {
        return skip("no .venv; run `uv sync`");
    }
    let program = Path::new("/usr/lib/libreoffice/program");
    let soffice = program.join("soffice.bin");
    if !soffice.is_file() {
        return skip("LibreOffice not installed in /usr/lib/libreoffice");
    }
    let tmp = tempfile::tempdir().unwrap();
    let adapters = tmp.path().join("adapters");
    std::fs::create_dir_all(adapters.join("office-probe")).unwrap();
    std::fs::write(adapters.join("office-probe/adapter.py"), OFFICE_SCRIPT).unwrap();
    let mut config = SandboxConfig::discover(
        &python,
        &repo_root().join("adapters"),
        Path::new(env!("CARGO_BIN_EXE_converter-sandbox-run")),
    )
    .expect("runtime discovery");
    config.adapters_dir = std::fs::canonicalize(&adapters).unwrap();
    // The grant app-core's `libreoffice_grant` gives on Linux.
    let mut read = vec![program.to_path_buf()];
    for dir in [
        "/usr/share/fonts",
        "/etc/fonts",
        "/etc/libreoffice",
        "/usr/lib/libreoffice",
    ] {
        if Path::new(dir).is_dir() {
            read.push(PathBuf::from(dir));
        }
    }
    config.grants.insert(
        "office-probe".into(),
        Grant {
            read,
            exec: vec![program.to_path_buf()],
            env: vec![("LFCS_BIN_SOFFICE".into(), soffice.clone().into_os_string())],
            processes: 3,
            ..Default::default()
        },
    );
    let launcher = SandboxLauncher::new(config);
    let job = tmp.path().join("j_office");
    for d in ["in", "out", "work"] {
        std::fs::create_dir_all(job.join(d)).unwrap();
    }
    let spec = LaunchSpec {
        adapter_id: "office-probe".into(),
        job_id: JobId::parse("j_office").unwrap(),
        job_dir: job,
        memory_limit: Some(2 << 30),
    };
    let out = launcher
        .command(&spec)
        .expect("worker command")
        .stdin(Stdio::null())
        .output()
        .expect("start the runner");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("PRODUCED ['note.pdf']"),
        "LibreOffice in the sandbox: {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
}
