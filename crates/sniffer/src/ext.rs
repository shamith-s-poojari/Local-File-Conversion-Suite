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
        "doc" | "dot" => Format::Doc,
        "odt" | "ott" => Format::Odt,
        "rtf" => Format::Rtf,
        "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" => Format::Pptx,
        "ppt" | "pps" | "pot" => Format::Ppt,
        "odp" | "otp" => Format::Odp,
        "xlsx" | "xlsm" | "xltx" | "xltm" => Format::Xlsx,
        "xls" | "xlt" => Format::Xls,
        "ods" | "ots" => Format::Ods,
        "csv" => Format::Csv,
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Format::Md,
        "html" | "htm" | "xhtml" => Format::Html,
        "txt" | "text" => Format::Txt,
        "epub" => Format::Epub,
        "json" => Format::Json,
        "yaml" | "yml" => Format::Yaml,
        "png" => Format::Png,
        "jpg" | "jpeg" | "jpe" | "jfif" => Format::Jpg,
        "webp" => Format::Webp,
        "svg" => Format::Svg,
        "heic" | "heif" | "hif" => Format::Heic,
        "gif" => Format::Gif,
        "bmp" | "dib" => Format::Bmp,
        "tif" | "tiff" => Format::Tiff,
        "avif" => Format::Avif,
        "ico" => Format::Ico,
        "mp3" => Format::Mp3,
        "wav" | "wave" => Format::Wav,
        "flac" => Format::Flac,
        "ogg" | "oga" => Format::Ogg,
        "opus" => Format::Opus,
        "m4a" | "m4b" => Format::M4a,
        "aac" => Format::Aac,
        "aif" | "aiff" | "aifc" => Format::Aiff,
        "mp4" | "m4v" => Format::Mp4,
        "mov" | "qt" => Format::Mov,
        "mkv" => Format::Mkv,
        "webm" => Format::Webm,
        "avi" => Format::Avi,
        _ => return None,
    })
}

/// The format a display name's extension claims.
pub fn claimed_format(display_name: &str) -> Option<Format> {
    extension(display_name).and_then(|e| format_for_extension(&e))
}
