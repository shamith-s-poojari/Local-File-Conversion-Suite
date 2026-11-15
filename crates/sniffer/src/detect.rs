use crate::ext::claimed_format;
use crate::source::Source;
use crate::{
    magic, text, zip, Confidence, Detection, EncryptionHint, Format, SniffError, PDF_MAGIC_WINDOW,
    WINDOW_BYTES,
};

/// One way the content can be read. Polyglots produce several.
struct Hit {
    format: Format,
    confidence: Confidence,
    /// Where the format's data starts; the earliest hit becomes the primary format.
    offset: u64,
}

const CFB_MAGIC: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: &[u8] = b"\xFF\xD8\xFF";
const HEIC_BRANDS: &[&[u8]] = &[b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx"];
const HEIF_GENERIC_BRANDS: &[&[u8]] = &[b"mif1", b"msf1"];

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn io_err(e: std::io::Error) -> SniffError {
    SniffError::from_io(&e)
}

/// Descriptor for an online-only file: nothing is read, so only the extension speaks.
pub fn placeholder(display_name: &str) -> Detection {
    Detection {
        format: claimed_format(display_name).unwrap_or(Format::Unknown),
        confidence: Confidence::Guess,
        evidence: vec!["placeholder:cloud".into()],
        extension_mismatch: false,
        candidates: Vec::new(),
        encryption_hint: EncryptionHint::None,
        macro_enabled: false,
    }
}

pub fn detect(display_name: &str, source: &mut impl Source) -> Result<Detection, SniffError> {
    let len = source.len();
    if len == 0 {
        return Err(SniffError::Empty);
    }
    let head_len = len.min(WINDOW_BYTES) as usize;
    let head = source.read_vec(0, head_len).map_err(io_err)?;
    let tail = if len <= WINDOW_BYTES {
        head.clone()
    } else {
        source
            .read_vec(len - WINDOW_BYTES, WINDOW_BYTES as usize)
            .map_err(io_err)?
    };

    let claimed = claimed_format(display_name);
    let mut hits: Vec<Hit> = Vec::new();
    let mut evidence: Vec<String> = Vec::new();
    let mut encryption_hint = EncryptionHint::None;
    let mut macro_enabled = false;

    // PDF: `%PDF-` anywhere in the first 1 KB (readers tolerate leading junk).
    let pdf_window = &head[..head.len().min(PDF_MAGIC_WINDOW)];
    if let Some(pos) = find(pdf_window, b"%PDF-") {
        let confidence = if pos == 0 {
            Confidence::Certain
        } else {
            Confidence::Likely
        };
        evidence.push(if pos == 0 {
            "magic:%PDF".into()
        } else {
            format!("magic:%PDF@{pos}")
        });
        if find(&tail, b"%%EOF").is_none() {
            evidence.push("pdf:no-eof".into());
        }
        if find(&head, b"/Encrypt").is_some() || find(&tail, b"/Encrypt").is_some() {
            encryption_hint = EncryptionHint::Maybe;
            evidence.push("pdf:encrypt-marker".into());
        }
        hits.push(Hit {
            format: Format::Pdf,
            confidence,
            offset: pos as u64,
        });
    }

    // Images, by magic at offset 0.
    if head.starts_with(PNG_MAGIC) {
        evidence.push("magic:png".into());
        hits.push(Hit {
            format: Format::Png,
            confidence: Confidence::Certain,
            offset: 0,
        });
    } else if head.starts_with(JPEG_MAGIC) {
        evidence.push("magic:jpeg".into());
        hits.push(Hit {
            format: Format::Jpg,
            confidence: Confidence::Certain,
            offset: 0,
        });
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        evidence.push("magic:riff-webp".into());
        hits.push(Hit {
            format: Format::Webp,
            confidence: Confidence::Certain,
            offset: 0,
        });
    } else if let Some((confidence, ev)) = heic(&head) {
        evidence.push(ev);
        hits.push(Hit {
            format: Format::Heic,
            confidence,
            offset: 0,
        });
    } else if let Some(m) = magic::image(&head)
        .or_else(|| magic::bmff(&head))
        .or_else(|| magic::media(&head))
    {
        evidence.push(m.evidence);
        hits.push(Hit {
            format: refine(m.format, claimed),
            confidence: m.confidence,
            offset: 0,
        });
    }

    // ZIP: either it starts with a local header, or an archive is appended (polyglot).
    let mut is_zip = false;
    if let Some(eocd) = zip::find_eocd(&tail) {
        is_zip = true;
        let finding = zip::inspect(source, &tail, eocd);
        evidence.push("magic:zip".into());
        evidence.extend(finding.evidence);
        macro_enabled |= finding.macro_enabled;
        if let (Some(format), Some(confidence)) = (finding.format, finding.confidence) {
            hits.push(Hit {
                format,
                confidence,
                offset: finding.archive_start,
            });
        }
    } else if head.starts_with(b"PK\x03\x04") {
        is_zip = true;
        evidence.push("magic:zip".into());
        evidence.push("zip:no-central-directory".into());
        // Truncated archive: only the extension can say which Office format it was.
        if zip::first_local_entry_is_content_types(&head) {
            if let Some(f @ (Format::Docx | Format::Pptx | Format::Xlsx)) = claimed {
                evidence.push("zip:first-entry:content-types".into());
                hits.push(Hit {
                    format: f,
                    confidence: Confidence::Guess,
                    offset: 0,
                });
            }
        }
    }

    // Compound File Binary: legacy Office, or OOXML encrypted with a password.
    if head.starts_with(CFB_MAGIC) {
        evidence.push("magic:cfb".into());
        let encrypted_ooxml = find(&head, &utf16le("EncryptionInfo")).is_some();
        if encrypted_ooxml {
            evidence.push("cfb:encrypted-ooxml".into());
            encryption_hint = EncryptionHint::Maybe;
            if let Some(f @ (Format::Docx | Format::Pptx | Format::Xlsx)) = claimed {
                hits.push(Hit {
                    format: f,
                    confidence: Confidence::Likely,
                    offset: 0,
                });
            }
        } else if let Some(m) = magic::legacy_office(source, &head, &tail) {
            evidence.push(m.evidence);
            hits.push(Hit {
                format: m.format,
                confidence: m.confidence,
                offset: 0,
            });
        } else if let Some(f @ (Format::Doc | Format::Xls | Format::Ppt)) = claimed {
            evidence.push("cfb:extension".into());
            hits.push(Hit {
                format: f,
                confidence: Confidence::Guess,
                offset: 0,
            });
        }
    }

    // Text formats only when nothing binary matched.
    if hits.is_empty() && !is_zip && !head.starts_with(CFB_MAGIC) {
        if let Some(t) = text::classify(&head, claimed) {
            evidence.extend(t.evidence);
            if t.format != Format::Unknown {
                hits.push(Hit {
                    format: t.format,
                    confidence: t.confidence,
                    offset: 0,
                });
            }
        } else {
            evidence.push("binary:unrecognized".into());
        }
    }

    hits.sort_by_key(|h| h.offset);
    let mut candidates: Vec<Format> = Vec::new();
    for h in &hits {
        if !candidates.contains(&h.format) {
            candidates.push(h.format);
        }
    }

    let (format, confidence) = match hits.first() {
        None => (Format::Unknown, Confidence::Guess),
        Some(h) if candidates.len() == 1 => (h.format, h.confidence),
        Some(h) => {
            evidence.push("polyglot".into());
            (h.format, Confidence::Likely)
        }
    };
    if candidates.len() < 2 {
        candidates.clear();
    }

    let extension_mismatch = match claimed {
        None => false,
        Some(c) if !candidates.is_empty() => !candidates.contains(&c),
        // Markdown and plain text are interchangeable and always overridable.
        Some(Format::Md | Format::Txt) if matches!(format, Format::Md | Format::Txt) => false,
        // MP4-family brands overlap: an `.m4v` or `.mp4` that is QuickTime is still playable.
        Some(Format::Mp4 | Format::Mov | Format::M4a)
            if matches!(format, Format::Mp4 | Format::Mov | Format::M4a) =>
        {
            false
        }
        Some(c) => c != format,
    };
    if extension_mismatch {
        evidence.push("extension-mismatch".into());
    }

    Ok(Detection {
        format,
        confidence,
        evidence,
        extension_mismatch,
        candidates,
        encryption_hint,
        macro_enabled,
    })
}

/// MP4 and M4A share brands: an MP4-branded file named `.m4a` is audio.
fn refine(format: Format, claimed: Option<Format>) -> Format {
    match (format, claimed) {
        (Format::Mp4, Some(Format::M4a)) => Format::M4a,
        _ => format,
    }
}

/// ISO BMFF `ftyp` box with a HEIC brand.
fn heic(head: &[u8]) -> Option<(Confidence, String)> {
    if head.len() < 16 || &head[4..8] != b"ftyp" {
        return None;
    }
    let box_len = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
    let major = &head[8..12];
    // Compatible brands follow the minor version, up to the end of the box.
    let compat_end = box_len.clamp(16, head.len());
    let compat: Vec<&[u8]> = head[16..compat_end]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| c.as_slice())
        .collect();

    if HEIC_BRANDS.contains(&major) {
        return Some((
            Confidence::Certain,
            format!("magic:ftyp:{}", String::from_utf8_lossy(major)),
        ));
    }
    if HEIF_GENERIC_BRANDS.contains(&major) {
        if compat.iter().any(|b| *b == b"avif" || *b == b"avis") {
            return None;
        }
        let confidence = if compat.iter().any(|b| HEIC_BRANDS.contains(b)) {
            Confidence::Certain
        } else {
            Confidence::Likely
        };
        return Some((
            confidence,
            format!("magic:ftyp:{}", String::from_utf8_lossy(major)),
        ));
    }
    None
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}
