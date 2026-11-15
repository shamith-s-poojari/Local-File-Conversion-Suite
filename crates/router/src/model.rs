use serde::{Deserialize, Serialize};

/// Every supported format (spec section 6, ADR 0003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
}

impl Format {
    /// Grouped by family (documents, images, audio, video), as the UI lists them.
    pub const ALL: [Format; 41] = [
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
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// Information level (blueprint section 3.3, extended by ADR 0003).
    pub fn level(self) -> Level {
        match self {
            Format::Docx
            | Format::Doc
            | Format::Odt
            | Format::Rtf
            | Format::Pptx
            | Format::Ppt
            | Format::Odp
            | Format::Xlsx
            | Format::Xls
            | Format::Ods
            | Format::Csv
            | Format::Md
            | Format::Html
            | Format::Txt
            | Format::Epub
            | Format::Json
            | Format::Yaml => Level::Structured,
            Format::Pdf | Format::Svg => Level::Fixed,
            Format::Png
            | Format::Jpg
            | Format::Webp
            | Format::Heic
            | Format::Gif
            | Format::Bmp
            | Format::Tiff
            | Format::Avif
            | Format::Ico => Level::Raster,
            Format::Mp3
            | Format::Wav
            | Format::Flac
            | Format::Ogg
            | Format::Opus
            | Format::M4a
            | Format::Aac
            | Format::Aiff => Level::Audio,
            Format::Mp4 | Format::Mov | Format::Mkv | Format::Webm | Format::Avi => Level::Video,
        }
    }

    /// One of the blueprint's original 12 formats, governed by its matrix.
    pub fn in_blueprint(self) -> bool {
        matches!(
            self,
            Format::Pdf
                | Format::Docx
                | Format::Pptx
                | Format::Xlsx
                | Format::Md
                | Format::Html
                | Format::Txt
                | Format::Png
                | Format::Jpg
                | Format::Webp
                | Format::Svg
                | Format::Heic
        )
    }
}

/// Moving down (Structured → Fixed → Raster) is normal; moving up is
/// reconstruction. Audio and video sit apart: they only convert among
/// themselves (and video to GIF), always directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Structured,
    Fixed,
    Raster,
    Audio,
    Video,
}

/// Fidelity tier (blueprint section 3.1).
///
/// Variants are declared **from best to worst**, so `Ord` ranks plans:
/// `E < H < R < L`. Lossy ranks last because a lossy hop throws content
/// away for good, while reconstruction keeps the content and only infers
/// structure. For example, DOCX → MD directly (R) must beat DOCX → TXT → MD,
/// whose worst hop is L. (Whether a target is *shown* by default is a
/// separate rule: R sits behind "Show best-effort".)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    E,
    H,
    R,
    L,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Linux
        }
    }
}
