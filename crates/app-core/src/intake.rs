//! Getting files into a selection from folders and the clipboard.
//!
//! - [`walk`] lists the files in a dropped folder: no links followed, no
//!   hidden files, no OS junk, bounded depth, in a stable order.
//! - [`paste`] checks and writes pasted bytes into the data root's `pasted/`
//!   folder under a random name; [`App`](crate::App) then sniffs it like any
//!   other file.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{CommandError, CommandResult};

/// Folders deeper than this below the dropped one are not entered.
pub const MAX_DEPTH: usize = 8;
/// Most files [`walk`] lists. Counting stops here, so a huge tree can't hang
/// the drop; the count of files left out is then a lower bound.
pub const MAX_WALKED: usize = 100_000;
/// Largest pasted item.
pub const MAX_PASTE_BYTES: usize = 50 * 1024 * 1024;
/// Where pasted files live, inside the data root.
pub const PASTED_DIR: &str = "pasted";

/// Files OS file managers drop into folders.
const JUNK: &[&str] = &[".ds_store", "thumbs.db", "desktop.ini", "ehthumbs.db"];

/// One file found in a dropped folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walked {
    pub path: PathBuf,
    /// Directories below the dropped folder, `/`-separated; empty at its top.
    pub relative_dir: String,
}

/// Everything under a dropped folder, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// The dropped folder's own name.
    pub root: String,
    pub files: Vec<Walked>,
    /// The tree had more than [`MAX_WALKED`] files; the rest were not listed.
    pub unlisted: bool,
}

impl Walked {
    /// `root/relative_dir`, or `root` at the top.
    pub fn folder(&self, root: &str) -> String {
        if self.relative_dir.is_empty() {
            root.to_owned()
        } else {
            format!("{root}/{}", self.relative_dir)
        }
    }
}

pub fn is_junk(name: &str) -> bool {
    name.starts_with('.') || JUNK.contains(&name.to_lowercase().as_str())
}

#[cfg(windows)]
fn hidden_attribute(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn hidden_attribute(_: &std::fs::Metadata) -> bool {
    false
}

/// Lists the regular files under `dir`. Skips symlinks (and Windows
/// junctions and other reparse points), hidden and system entries, dot
/// names and OS junk, and anything deeper than [`MAX_DEPTH`]. Unreadable
/// folders are skipped. Each folder is read in name order, files and
/// folders interleaved, so the result is the same on every run.
pub fn walk(dir: &Path) -> Walk {
    let root = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "folder".into());
    let mut out = Walk {
        root,
        files: Vec::new(),
        unlisted: false,
    };
    walk_into(dir, "", 0, &mut out);
    out
}

fn walk_into(dir: &Path, relative: &str, depth: usize, out: &mut Walk) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(String, PathBuf)> = read
        .filter_map(Result::ok)
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    entries.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then_with(|| a.0.cmp(&b.0))
    });
    for (name, path) in entries {
        if is_junk(&name) {
            continue;
        }
        // `symlink_metadata`: a link is never followed, whatever it points at.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || hidden_attribute(&meta) {
            continue;
        }
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                let below = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                walk_into(&path, &below, depth + 1, out);
            }
        } else if meta.is_file() {
            if out.files.len() >= MAX_WALKED {
                out.unlisted = true;
                return;
            }
            out.files.push(Walked {
                path,
                relative_dir: relative.to_owned(),
            });
        }
        if out.unlisted {
            return;
        }
    }
}

// ---------------------------------------------------------------- pasting

/// What was pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteKind {
    Image,
    Table,
    Text,
}

impl PasteKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "image" => Some(Self::Image),
            "table" => Some(Self::Table),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

/// A pasted item written to disk.
#[derive(Debug)]
pub struct Pasted {
    pub path: PathBuf,
    /// What the user sees: `pasted-image.png`, `pasted-table.csv`, `pasted-text.md`.
    pub display_name: String,
}

/// Checks pasted bytes and writes them to `dir` under a random name.
/// Images must be PNG or JPEG (by their first bytes); tables and text must be
/// UTF-8 text.
pub fn paste(dir: &Path, kind: PasteKind, bytes: &[u8]) -> CommandResult<Pasted> {
    if bytes.is_empty() {
        return Err(CommandError::new("E_INPUT_CORRUPT", "emptyPaste"));
    }
    if bytes.len() > MAX_PASTE_BYTES {
        return Err(CommandError::new("E_LIMIT_EXCEEDED", "pasteTooLarge"));
    }
    let (ext, display_name) = match kind {
        PasteKind::Image if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => ("png", "pasted-image.png"),
        PasteKind::Image if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) => ("jpg", "pasted-image.jpg"),
        PasteKind::Image => return Err(CommandError::new("E_INPUT_CORRUPT", "notAnImage")),
        PasteKind::Table | PasteKind::Text => {
            if std::str::from_utf8(bytes).is_err() {
                return Err(CommandError::new("E_INPUT_CORRUPT", "notText"));
            }
            match kind {
                PasteKind::Table => ("csv", "pasted-table.csv"),
                _ => ("md", "pasted-text.md"),
            }
        }
    };
    std::fs::create_dir_all(dir)
        .map_err(|_| CommandError::new("E_RESOURCE_DISK", "pasteFolder"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    for _ in 0..8 {
        let path = dir.join(format!("{}.{ext}", random_name()));
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        match created {
            Ok(mut f) => {
                if f.write_all(bytes).and_then(|()| f.flush()).is_err() {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(CommandError::new("E_RESOURCE_DISK", "pasteWrite"));
                }
                return Ok(Pasted {
                    path,
                    display_name: display_name.to_owned(),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(CommandError::new("E_RESOURCE_DISK", "pasteWrite")),
        }
    }
    Err(CommandError::new("E_INTERNAL", "pasteName"))
}

/// 128 random bits as hex, from the std hasher's per-instance random keys.
fn random_name() -> String {
    let a = RandomState::new().build_hasher().finish();
    let b = RandomState::new().build_hasher().finish();
    format!("p_{a:016x}{b:016x}")
}

/// Deletes everything in the pasted folder (app start and exit).
pub fn clear_pasted(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn rels(w: &Walk) -> Vec<String> {
        w.files
            .iter()
            .map(|f| {
                let name = f.path.file_name().unwrap().to_string_lossy();
                if f.relative_dir.is_empty() {
                    name.into_owned()
                } else {
                    format!("{}/{name}", f.relative_dir)
                }
            })
            .collect()
    }

    #[test]
    fn walks_in_sorted_order_with_relative_dirs() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("photos");
        for f in ["b.png", "a.png", "2024/z.png", "2024/trip/c.png", "Zed.png"] {
            touch(&root.join(f));
        }
        let w = walk(&root);
        assert_eq!(w.root, "photos");
        assert_eq!(
            rels(&w),
            ["2024/trip/c.png", "2024/z.png", "a.png", "b.png", "Zed.png"]
        );
        assert_eq!(w.files[0].folder("photos"), "photos/2024/trip");
        assert_eq!(w.files[2].folder("photos"), "photos");
        assert!(!w.unlisted);
    }

    #[test]
    fn skips_hidden_and_junk() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("r");
        for f in [
            "keep.txt",
            ".hidden.txt",
            ".git/config",
            ".DS_Store",
            "Thumbs.db",
            "desktop.ini",
            "sub/.secret",
            "sub/ok.txt",
        ] {
            touch(&root.join(f));
        }
        assert_eq!(rels(&walk(&root)), ["keep.txt", "sub/ok.txt"]);
    }

    #[test]
    fn stops_at_the_depth_limit() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("deep");
        let mut dir = root.clone();
        touch(&dir.join("level0.txt"));
        for i in 1..=MAX_DEPTH + 2 {
            dir = dir.join(format!("d{i}"));
            touch(&dir.join(format!("level{i}.txt")));
        }
        let w = walk(&root);
        assert_eq!(w.files.len(), MAX_DEPTH + 1);
        let deepest = w
            .files
            .iter()
            .map(|f| f.relative_dir.split('/').filter(|c| !c.is_empty()).count())
            .max();
        assert_eq!(deepest, Some(MAX_DEPTH));
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let outside = t.path().join("outside");
        touch(&outside.join("secret.txt"));
        let root = t.path().join("r");
        touch(&root.join("real.txt"));
        std::os::unix::fs::symlink(&outside, root.join("linked-dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("linked.txt")).unwrap();
        // A loop must not hang or repeat.
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        assert_eq!(rels(&walk(&root)), ["real.txt"]);
    }

    #[test]
    fn unreadable_or_missing_folder_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(walk(&t.path().join("missing")).files.is_empty());
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n rest";

    #[test]
    fn pastes_get_random_names_and_display_names() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(PASTED_DIR);
        let a = paste(&dir, PasteKind::Image, PNG).unwrap();
        let b = paste(&dir, PasteKind::Image, PNG).unwrap();
        assert_ne!(a.path, b.path);
        assert_eq!(a.display_name, "pasted-image.png");
        assert_eq!(a.path.extension().unwrap(), "png");
        assert_eq!(fs::read(&a.path).unwrap(), PNG);
        let j = paste(&dir, PasteKind::Image, &[0xFF, 0xD8, 0xFF, 0xE0, 1]).unwrap();
        assert_eq!(j.display_name, "pasted-image.jpg");
        let c = paste(&dir, PasteKind::Table, b"a,b\n1,2\n").unwrap();
        assert_eq!(c.display_name, "pasted-table.csv");
        assert_eq!(c.path.extension().unwrap(), "csv");
        let m = paste(&dir, PasteKind::Text, "héllo".as_bytes()).unwrap();
        assert_eq!(m.display_name, "pasted-text.md");
        assert_eq!(m.path.extension().unwrap(), "md");
        // The name never comes from the content.
        assert!(!m
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("llo"));
    }

    #[test]
    fn refuses_bad_pastes() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(PASTED_DIR);
        let err = |r: CommandResult<Pasted>| r.unwrap_err().reason.unwrap();
        assert_eq!(
            err(paste(&dir, PasteKind::Image, b"GIF89a....")),
            "notAnImage"
        );
        assert_eq!(err(paste(&dir, PasteKind::Image, b"")), "emptyPaste");
        assert_eq!(
            err(paste(&dir, PasteKind::Text, &[0xFF, 0xFE, 0x00])),
            "notText"
        );
        assert_eq!(err(paste(&dir, PasteKind::Table, &[0xC3])), "notText");
        let big = vec![b'a'; MAX_PASTE_BYTES + 1];
        assert_eq!(err(paste(&dir, PasteKind::Text, &big)), "pasteTooLarge");
        assert!(!dir.exists() || fs::read_dir(&dir).unwrap().count() == 0);
    }

    #[test]
    fn clear_removes_the_folder() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(PASTED_DIR);
        paste(&dir, PasteKind::Text, b"x").unwrap();
        clear_pasted(&dir);
        assert!(!dir.exists());
    }
}
