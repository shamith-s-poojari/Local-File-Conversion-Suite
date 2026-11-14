//! File format sniffer (spec section 6, blueprint section 2).
//!
//! The sniffer identifies every supported format from content, using the
//! extension only as a tie-breaker. It reads at most:
//!
//! - the first and last [`WINDOW_BYTES`] of the file,
//! - the ZIP central directory, capped at [`MAX_CENTRAL_DIRECTORY_BYTES`],
//! - `[Content_Types].xml`, inflated to at most [`MAX_CONTENT_TYPES_BYTES`].
//!
//! It never decodes any other content. Deep inspection (page counts,
//! passwords, validity) happens later inside a sandboxed worker, so every
//! answer here is a hint the worker may overrule.
//!
//! The sniffer must never panic on any input; `fuzz/` holds the fuzz target.

mod detect;
mod ext;
mod magic;
mod placeholder;
mod source;
mod text;
mod zip;

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

pub use source::{FileSource, SliceSource, Source};

/// Bytes read from each end of the file.
pub const WINDOW_BYTES: u64 = 64 * 1024;
/// `%PDF-` must start within this many bytes.
pub const PDF_MAGIC_WINDOW: usize = 1024;
/// Largest ZIP central directory the sniffer will read.
pub const MAX_CENTRAL_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;
/// Largest `[Content_Types].xml` the sniffer will read or inflate.
pub const MAX_CONTENT_TYPES_BYTES: usize = 1024 * 1024;

/// Every format the suite reads or writes, plus `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Pdf,
    Docx,
    Doc,
    Odt,
    Rtf,
    Pptx,
    Ppt,
    Odp,
    Xlsx,
    Xls,
    Ods,
    Csv,
    Md,
    Html,
    Txt,
    Epub,
    Json,
    Yaml,
    Png,
    Jpg,
    Webp,
    Svg,
    Heic,
    Gif,
    Bmp,
    Tiff,
    Avif,
    Ico,
    Mp3,
    Wav,
    Flac,
    Ogg,
    Opus,
    M4a,
    Aac,
    Aiff,
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
    Unknown,
}

impl Format {
    /// Every known format (not `Unknown`).
    pub const KNOWN: [Format; 41] = [
        Format::Pdf,
        Format::Docx,
        Format::Doc,
        Format::Odt,
        Format::Rtf,
        Format::Pptx,
        Format::Ppt,
        Format::Odp,
        Format::Xlsx,
        Format::Xls,
        Format::Ods,
        Format::Csv,
        Format::Md,
        Format::Html,
        Format::Txt,
        Format::Epub,
        Format::Json,
        Format::Yaml,
        Format::Png,
        Format::Jpg,
        Format::Webp,
        Format::Svg,
        Format::Heic,
        Format::Gif,
        Format::Bmp,
        Format::Tiff,
        Format::Avif,
        Format::Ico,
        Format::Mp3,
        Format::Wav,
        Format::Flac,
        Format::Ogg,
        Format::Opus,
        Format::M4a,
        Format::Aac,
        Format::Aiff,
        Format::Mp4,
        Format::Mov,
        Format::Mkv,
        Format::Webm,
        Format::Avi,
    ];

    /// The format with this wire name, if known.
    pub fn parse(s: &str) -> Option<Format> {
        Format::KNOWN.into_iter().find(|f| f.as_str() == s)
    }

    /// Lowercase name, as serialized.
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Doc => "doc",
            Format::Odt => "odt",
            Format::Rtf => "rtf",
            Format::Pptx => "pptx",
            Format::Ppt => "ppt",
            Format::Odp => "odp",
            Format::Xlsx => "xlsx",
            Format::Xls => "xls",
            Format::Ods => "ods",
            Format::Csv => "csv",
            Format::Md => "md",
            Format::Html => "html",
            Format::Txt => "txt",
            Format::Epub => "epub",
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Webp => "webp",
            Format::Svg => "svg",
            Format::Heic => "heic",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
            Format::Avif => "avif",
            Format::Ico => "ico",
            Format::Mp3 => "mp3",
            Format::Wav => "wav",
            Format::Flac => "flac",
            Format::Ogg => "ogg",
            Format::Opus => "opus",
            Format::M4a => "m4a",
            Format::Aac => "aac",
            Format::Aiff => "aiff",
            Format::Mp4 => "mp4",
            Format::Mov => "mov",
            Format::Mkv => "mkv",
            Format::Webm => "webm",
            Format::Avi => "avi",
            Format::Unknown => "unknown",
        }
    }
}

/// How sure the sniffer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Guess,
    Likely,
    Certain,
}

/// Only a hint: `inspect` in the worker gives the authoritative answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EncryptionHint {
    None,
    Maybe,
}

/// What the content says, before path and identity are added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub format: Format,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    pub extension_mismatch: bool,
    /// Non-empty when the file is valid as more than one format.
    pub candidates: Vec<Format>,
    pub encryption_hint: EncryptionHint,
    pub macro_enabled: bool,
}

/// Result of sniffing one file (spec section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDescriptor {
    /// Random; never derived from the path.
    pub file_id: String,
    /// Untrusted text: render it as text, never as markup.
    pub display_name: String,
    pub size_bytes: u64,
    pub format: Format,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    pub extension_mismatch: bool,
    pub candidates: Vec<Format>,
    pub encryption_hint: EncryptionHint,
    pub macro_enabled: bool,
    pub cloud_placeholder: bool,
    /// `true` for unambiguous, local files in a known format.
    pub actionable: bool,
    /// For a file that came from a dropped folder: that folder's name, then
    /// the directories below it, with `/` separators (`photos/2024/trip`).
    /// Set by the app core; never an absolute path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl FileDescriptor {
    fn new(display_name: String, size_bytes: u64, d: Detection, cloud_placeholder: bool) -> Self {
        let actionable =
            d.format != Format::Unknown && d.candidates.is_empty() && !cloud_placeholder;
        FileDescriptor {
            file_id: new_file_id(),
            display_name,
            size_bytes,
            format: d.format,
            confidence: d.confidence,
            evidence: d.evidence,
            extension_mismatch: d.extension_mismatch,
            candidates: d.candidates,
            encryption_hint: d.encryption_hint,
            macro_enabled: d.macro_enabled,
            cloud_placeholder,
            actionable,
            folder: None,
        }
    }
}

/// Why a file was rejected. Per-file: other files continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SniffError {
    /// Zero bytes.
    Empty,
    Directory,
    /// Device, pipe, socket or anything else that isn't a regular file.
    SpecialFile,
    NotFound,
    PermissionDenied,
    /// Another program holds a lock on the file.
    Locked,
    Unreadable,
}

impl SniffError {
    fn from_io(e: &io::Error) -> Self {
        // Windows: ERROR_SHARING_VIOLATION (32) and ERROR_LOCK_VIOLATION (33).
        if cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)) {
            return SniffError::Locked;
        }
        match e.kind() {
            io::ErrorKind::NotFound => SniffError::NotFound,
            io::ErrorKind::PermissionDenied => SniffError::PermissionDenied,
            io::ErrorKind::IsADirectory => SniffError::Directory,
            _ => SniffError::Unreadable,
        }
    }
}

impl fmt::Display for SniffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SniffError::Empty => "the file is empty",
            SniffError::Directory => "this is a folder, not a file",
            SniffError::SpecialFile => "this is not a regular file",
            SniffError::NotFound => "the file no longer exists",
            SniffError::PermissionDenied => "you don't have permission to read this file",
            SniffError::Locked => "another program is using this file",
            SniffError::Unreadable => "the file could not be read",
        })
    }
}

impl std::error::Error for SniffError {}

/// Sniff a file on disk. Cloud placeholders are not read, so reading doesn't
/// trigger a download; they come back with `cloud_placeholder` set.
pub fn sniff_path(path: &Path) -> Result<FileDescriptor, SniffError> {
    let display_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let meta = fs::metadata(path).map_err(|e| SniffError::from_io(&e))?;
    if meta.is_dir() {
        return Err(SniffError::Directory);
    }
    if !meta.is_file() {
        return Err(SniffError::SpecialFile);
    }
    if placeholder::is_cloud_placeholder(&meta) {
        let d = detect::placeholder(&display_name);
        return Ok(FileDescriptor::new(display_name, meta.len(), d, true));
    }
    if meta.len() == 0 {
        return Err(SniffError::Empty);
    }

    let file = fs::File::open(path).map_err(|e| SniffError::from_io(&e))?;
    let mut source = FileSource::new(file).map_err(|e| SniffError::from_io(&e))?;
    let size = source.len();
    let d = detect::detect(&display_name, &mut source)?;
    Ok(FileDescriptor::new(display_name, size, d, false))
}

/// Sniff in-memory bytes as if they were a file called `display_name`.
pub fn sniff_bytes(display_name: &str, bytes: &[u8]) -> Result<FileDescriptor, SniffError> {
    let d = detect_bytes(display_name, bytes)?;
    Ok(FileDescriptor::new(
        display_name.to_owned(),
        bytes.len() as u64,
        d,
        false,
    ))
}

/// Content detection only, without a file ID. Used by tests and the fuzz target.
pub fn detect_bytes(display_name: &str, bytes: &[u8]) -> Result<Detection, SniffError> {
    detect::detect(display_name, &mut SliceSource::new(bytes))
}

fn new_file_id() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("the OS random number generator is unavailable");
    let mut id = String::with_capacity(34);
    id.push_str("f_");
    for b in buf {
        id.push_str(&format!("{b:02x}"));
    }
    id
}
