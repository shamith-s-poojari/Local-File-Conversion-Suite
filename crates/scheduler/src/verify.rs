//! Checks on a worker's `executeResult` (spec section 7.4).

use std::fs::File;
use std::io::Read;
use std::path::Path;

use converter_jobstore::{JobFailure, JobId, JobStore, OutputFile};
use converter_sniffer::Format;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::worker::failure;

/// Records and checks every output the worker claims. Run it after the
/// worker has exited, so it can't change the files during the checks.
///
/// - Each path must be a regular file inside `out/`, with no links or
///   reparse points, and the claimed size: otherwise `E_PROTOCOL`.
/// - The SHA-256 must match the file: otherwise `E_PROTOCOL`.
/// - The file must sniff as the declared format: otherwise
///   `E_OUTPUT_INVALID`.
///
/// On failure the caller fails the job, which deletes `out/`.
pub fn verify_outputs(
    store: &JobStore,
    job: &JobId,
    result: &Value,
    format: Format,
) -> Result<(), JobFailure> {
    let claimed = result["outputs"].as_array().cloned().unwrap_or_default();
    if claimed.is_empty() {
        return Err(failure("E_PROTOCOL"));
    }
    let mut files = Vec::with_capacity(claimed.len());
    let mut hashes = Vec::with_capacity(claimed.len());
    for o in &claimed {
        // The schema guarantees `out/...`; check anyway rather than panic.
        let Some(rel) = o["path"].as_str().and_then(|p| p.strip_prefix("out/")) else {
            return Err(failure("E_PROTOCOL"));
        };
        files.push(OutputFile {
            path: rel.to_owned(),
            suggested_name: o["suggestedName"].as_str().unwrap_or_default().to_owned(),
            extension: format.as_str().to_owned(),
            size_bytes: o["sizeBytes"].as_u64().unwrap_or(u64::MAX),
        });
        hashes.push(o["sha256"].as_str().unwrap_or_default().to_owned());
    }
    store
        .record_outputs(job, files.clone())
        .map_err(|_| failure("E_PROTOCOL"))?;

    let out = store.job_dirs(job).map_err(|_| failure("E_INTERNAL"))?.out;
    for (file, hash) in files.iter().zip(&hashes) {
        let path = out.join(&file.path);
        let actual = sha256_hex(&path).map_err(|_| failure("E_PROTOCOL"))?;
        if !actual.eq_ignore_ascii_case(hash) {
            return Err(failure("E_PROTOCOL"));
        }
        let sniffed =
            converter_sniffer::sniff_path(&path).map_err(|_| failure("E_OUTPUT_INVALID"))?;
        if sniffed.format != format {
            return Err(failure("E_OUTPUT_INVALID"));
        }
    }
    Ok(())
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
