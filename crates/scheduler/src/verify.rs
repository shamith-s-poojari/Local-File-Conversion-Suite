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
/// - The file must sniff as the format its part declares: otherwise
///   `E_OUTPUT_INVALID`.
///
/// On failure the caller fails the job, which deletes `out/`.
pub fn verify_outputs(
    store: &JobStore,
    job: &JobId,
    claimed: &[(Value, Format)],
) -> Result<(), JobFailure> {
    if claimed.is_empty() {
        return Err(failure("E_PROTOCOL"));
    }
    let mut files = Vec::with_capacity(claimed.len());
    let mut hashes = Vec::with_capacity(claimed.len());
    let mut formats = Vec::with_capacity(claimed.len());
    for (o, format) in claimed {
        // The schema guarantees `out/...`; check anyway rather than panic.
        let Some(rel) = o["path"].as_str().and_then(|p| p.strip_prefix("out/")) else {
            return Err(failure("E_PROTOCOL"));
        };
        let hash = o["sha256"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        files.push(OutputFile {
            path: rel.to_owned(),
            suggested_name: o["suggestedName"].as_str().unwrap_or_default().to_owned(),
            extension: format.as_str().to_owned(),
            size_bytes: o["sizeBytes"].as_u64().unwrap_or(u64::MAX),
            source: o["source"].as_u64().and_then(|i| usize::try_from(i).ok()),
            // Checked against the file just below; saving checks it again
            // while copying, which closes the gap after this check.
            sha256: Some(hash.clone()),
        });
        hashes.push(hash);
        formats.push(*format);
    }
    store
        .record_outputs(job, files.clone())
        .map_err(|_| failure("E_PROTOCOL"))?;

    let out = store.job_dirs(job).map_err(|_| failure("E_INTERNAL"))?.out;
    for ((file, hash), format) in files.iter().zip(&hashes).zip(formats) {
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

/// Opens a file for reading without following a link or blocking on a
/// FIFO, and refuses anything that isn't a regular file. An earlier path
/// check can't be relied on, since the path can change after it; the open
/// is the check that can't be raced. Shared so every place that reads a
/// worker's file opens it the same way.
pub fn open_regular(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    // Windows: open a reparse point (symlink, junction) itself, not its
    // target, so the metadata check below sees the link and refuses it.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let f = options.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(f)
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut f = open_regular(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(hasher))
}

fn hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Copies `source` to a new file `dest` and checks the bytes copied against
/// `expected` (hex SHA-256). `dest` is created exclusively, so it can't be a
/// link or a file that already exists; on a mismatch it is deleted.
pub(crate) fn copy_verified(source: &Path, dest: &Path, expected: &str) -> std::io::Result<()> {
    let mut src = open_regular(source)?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    let copied = (|| {
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                return Ok(hex(hasher));
            }
            hasher.update(&buf[..n]);
            std::io::Write::write_all(&mut out, &buf[..n])?;
        }
    })();
    let result = match copied {
        Ok(actual) if actual.eq_ignore_ascii_case(expected) => Ok(()),
        Ok(_) => Err(std::io::Error::other("changedAfterCheck")),
        Err(e) => Err(e),
    };
    if result.is_err() {
        drop(out);
        let _ = std::fs::remove_file(dest);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn hashing_refuses_links_and_fifos_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let link = dir.path().join("l");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(sha256_hex(&link).is_err());
        let fifo = dir.path().join("p");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        // Would hang forever without O_NONBLOCK.
        assert!(sha256_hex(&fifo).is_err());
    }

    #[test]
    fn a_copy_that_differs_from_the_recorded_hash_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let ok = dir.path().join("ok");
        copy_verified(&file, &ok, abc).unwrap();
        assert_eq!(std::fs::read(&ok).unwrap(), b"abc");
        // Same length, other bytes: the case a size check can't see.
        std::fs::write(&file, b"abd").unwrap();
        let bad = dir.path().join("bad");
        assert!(copy_verified(&file, &bad, abc).is_err());
        assert!(!bad.exists());
        // An existing destination is never overwritten.
        assert!(copy_verified(&file, &ok, abc).is_err());
        assert_eq!(std::fs::read(&ok).unwrap(), b"abc");
    }
}
