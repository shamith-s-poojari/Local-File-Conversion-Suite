use std::path::Path;

use crate::Format;

/// Lowercased extension of a display name, without the dot.
pub fn extension(display_name: &str) -> Option<String> {
    Path::new(display_name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
}

/// The format an extension claims. Variants (templates, macro-enabled) map to
/// their base format; the content decides the rest.
pub fn format_for_extension(ext: &str) -> Option<Format> {
    Some(match ext {
        "pdf" => Format::Pdf,
        "docx" | "docm" | "dotx" | "dotm" => Format::Docx,
        "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" => Format::Pptx,
        "xlsx" | "xlsm" | "xltx" | "xltm" => Format::Xlsx,
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Format::Md,
        "html" | "htm" | "xhtml" => Format::Html,
        "txt" | "text" => Format::Txt,
        "png" => Format::Png,
        "jpg" | "jpeg" | "jpe" | "jfif" => Format::Jpg,
        "webp" => Format::Webp,
        "svg" => Format::Svg,
        "heic" | "heif" | "hif" => Format::Heic,
        _ => return None,
    })
}

/// The format a display name's extension claims.
pub fn claimed_format(display_name: &str) -> Option<Format> {
    extension(display_name).and_then(|e| format_for_extension(&e))
}
