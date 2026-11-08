use serde::{Deserialize, Serialize};

/// The 12 planned formats (spec section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Md,
    Html,
    Txt,
    Png,
    Jpg,
    Webp,
    Svg,
    Heic,
}

impl Format {
    /// In the blueprint's matrix order.
    pub const ALL: [Format; 12] = [
        Format::Pdf,
        Format::Docx,
        Format::Pptx,
        Format::Xlsx,
        Format::Md,
        Format::Html,
        Format::Txt,
        Format::Png,
        Format::Jpg,
        Format::Webp,
        Format::Svg,
        Format::Heic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Pptx => "pptx",
            Format::Xlsx => "xlsx",
            Format::Md => "md",
            Format::Html => "html",
            Format::Txt => "txt",
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Webp => "webp",
            Format::Svg => "svg",
            Format::Heic => "heic",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// Information level (blueprint section 3.3).
    pub fn level(self) -> Level {
        match self {
            Format::Docx
            | Format::Pptx
            | Format::Xlsx
            | Format::Md
            | Format::Html
            | Format::Txt => Level::Structured,
            Format::Pdf | Format::Svg => Level::Fixed,
            Format::Png | Format::Jpg | Format::Webp | Format::Heic => Level::Raster,
        }
    }
}

/// Moving down (Structured → Fixed → Raster) is normal; moving up is reconstruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Structured,
    Fixed,
    Raster,
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
