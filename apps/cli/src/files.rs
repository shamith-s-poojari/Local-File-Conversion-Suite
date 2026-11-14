//! Expanding the command line's paths into files, and finding what a save wrote.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One input file and, when it came from a folder, where in that folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub path: PathBuf,
    /// The folder's name and the directories below it, with `/` separators.
    pub layout: Option<String>,
    /// Came from a folder walk (so unrecognised files are skipped quietly).
    pub from_folder: bool,
}

/// Files stay as given; folders become their files, sorted, depth first.
/// Dot files and symbolic links inside folders are skipped.
pub fn expand(paths: &[String]) -> Result<Vec<Input>, String> {
    let mut out = Vec::new();
    for p in paths {
        let path = PathBuf::from(p);
        if path.is_dir() {
            let name = folder_name(&path);
            walk(&path, &name, &mut out)?;
        } else {
            out.push(Input {
                path,
                layout: None,
                from_folder: false,
            });
        }
    }
    Ok(out)
}

fn folder_name(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .or_else(|| path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "folder".into())
}

fn walk(dir: &Path, layout: &str, out: &mut Vec<Input>) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(kind) = e.file_type() else { continue };
        if name.starts_with('.') || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&e.path(), &format!("{layout}/{name}"), out)?;
        } else if kind.is_file() {
            out.push(Input {
                path: e.path(),
                layout: Some(layout.to_string()),
                from_folder: true,
            });
        }
    }
    Ok(())
}

/// Files in `dir` (top level), and everything under the folders named in
/// `descend`. Used before and after a save to see what it wrote.
pub fn listing(dir: &Path, descend: &BTreeSet<String>) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let Ok(read) = fs::read_dir(dir) else {
        return found;
    };
    for e in read.filter_map(Result::ok) {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if descend.contains(&e.file_name().to_string_lossy().into_owned()) {
                all_files(&e.path(), &mut found, 0);
            }
        } else {
            found.insert(e.path());
        }
    }
    found
}

fn all_files(dir: &Path, found: &mut BTreeSet<PathBuf>, depth: usize) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for e in read.filter_map(Result::ok) {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() && depth < 16 {
            all_files(&e.path(), found, depth + 1);
        } else if kind.is_file() {
            found.insert(e.path());
        }
    }
}

/// Names of the directories directly in `dir`.
pub fn subdirs(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|k| k.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_expand_sorted_with_layout() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("photos");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("b.png"), b"x").unwrap();
        fs::write(d.join("a.png"), b"x").unwrap();
        fs::write(d.join(".hidden"), b"x").unwrap();
        fs::write(d.join("sub/c.png"), b"x").unwrap();
        let loose = t.path().join("loose.png");
        fs::write(&loose, b"x").unwrap();
        let got = expand(&[
            d.to_string_lossy().into_owned(),
            loose.to_string_lossy().into_owned(),
        ])
        .unwrap();
        let layouts: Vec<_> = got.iter().map(|i| i.layout.as_deref()).collect();
        assert_eq!(
            layouts,
            [Some("photos"), Some("photos"), Some("photos/sub"), None]
        );
        assert!(got[0].path.ends_with("a.png"));
        assert!(!got[3].from_folder);
    }
}
