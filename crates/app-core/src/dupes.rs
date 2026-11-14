//! Identical files in the selection (ADR 0005, smart suggestions).
//!
//! Only files of the same size are hashed, streaming with SHA-256. Nothing
//! is parsed, so this runs in the core like the sniffer's magic-byte reads.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// How many files repeat an earlier one: three identical files count 2.
/// A file that can't be read counts as unique.
pub fn count_duplicates(files: &[(u64, PathBuf)]) -> usize {
    let mut by_size: HashMap<u64, Vec<&Path>> = HashMap::new();
    for (size, path) in files {
        by_size.entry(*size).or_default().push(path);
    }
    let mut repeats = 0;
    for paths in by_size.values().filter(|p| p.len() > 1) {
        let mut seen = HashSet::new();
        for path in paths {
            if let Ok(hash) = sha256(path) {
                if !seen.insert(hash) {
                    repeats += 1;
                }
            }
        }
    }
    repeats
}

fn sha256(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> (u64, PathBuf) {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        (bytes.len() as u64, path)
    }

    #[test]
    fn counts_repeats_not_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let files = vec![
            write(d, "a", b"hello"),
            write(d, "b", b"hello"),
            write(d, "c", b"hello"),
            write(d, "d", b"world"), // same size, different bytes
            write(d, "e", b"other size"),
        ];
        assert_eq!(count_duplicates(&files), 2);
    }

    #[test]
    fn unreadable_files_are_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![(5, tmp.path().join("gone")), (5, tmp.path().join("gone2"))];
        assert_eq!(count_duplicates(&files), 0);
        assert_eq!(count_duplicates(&[]), 0);
    }
}
