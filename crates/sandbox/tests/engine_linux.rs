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
