//! OOXML detection from the ZIP central directory and `[Content_Types].xml`.
//! Nothing else in the archive is read or inflated.

use crate::source::Source;
use crate::{Confidence, Format, MAX_CENTRAL_DIRECTORY_BYTES, MAX_CONTENT_TYPES_BYTES};

const EOCD_SIG: &[u8; 4] = b"PK\x05\x06";
const EOCD_LEN: usize = 22;
const ZIP64_LOCATOR_SIG: &[u8; 4] = b"PK\x06\x07";
const ZIP64_EOCD_SIG: &[u8; 4] = b"PK\x06\x06";
const CD_ENTRY_SIG: &[u8; 4] = b"PK\x01\x02";
const LOCAL_SIG: &[u8; 4] = b"PK\x03\x04";
const CONTENT_TYPES: &[u8] = b"[Content_Types].xml";
/// Stop listing names after this many entries; OOXML main parts come early.
const MAX_ENTRIES: usize = 200_000;

#[derive(Debug, Default)]
pub struct ZipFinding {
    /// `None` for a ZIP that isn't OOXML.
    pub format: Option<Format>,
    pub confidence: Option<Confidence>,
    pub macro_enabled: bool,
    pub evidence: Vec<String>,
    /// Where the archive starts in the file. Non-zero for appended archives.
    pub archive_start: u64,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Position of the end-of-central-directory record in `tail`, if any.
pub fn find_eocd(tail: &[u8]) -> Option<usize> {
    if tail.len() < EOCD_LEN {
        return None;
    }
    (0..=tail.len() - EOCD_LEN).rev().find(|&i| {
        &tail[i..i + 4] == EOCD_SIG
            && u16_at(tail, i + 20)
                .is_some_and(|comment| i + EOCD_LEN + usize::from(comment) <= tail.len())
    })
}

struct Entry {
    name: Vec<u8>,
    method: u16,
    compressed_size: u64,
    uncompressed_size: u64,
    local_offset: u64,
}

/// Inspect a ZIP whose EOCD record is at `tail[eocd]`.
pub fn inspect(source: &mut impl Source, tail: &[u8], eocd: usize) -> ZipFinding {
    let mut f = ZipFinding::default();
    let tail_start = source.len() - tail.len() as u64;
    let eocd_abs = tail_start + eocd as u64;

    let Some((cd_offset, cd_size, cd_end)) =
        central_directory_location(source, tail, eocd, eocd_abs)
    else {
        f.evidence.push("zip:bad-eocd".into());
        return f;
    };
    // Data prepended to the archive shifts every stored offset by the same amount.
    let Some(cd_start) = cd_end.checked_sub(cd_size) else {
        f.evidence.push("zip:bad-eocd".into());
        return f;
    };
    let Some(shift) = cd_start.checked_sub(cd_offset) else {
        f.evidence.push("zip:bad-eocd".into());
        return f;
    };
    f.archive_start = shift;
    if cd_size > MAX_CENTRAL_DIRECTORY_BYTES {
        f.evidence.push("zip:central-directory-too-large".into());
        return f;
    }
    let Ok(cd) = source.read_vec(cd_start, cd_size as usize) else {
        f.evidence.push("zip:bad-central-directory".into());
        return f;
    };
    let entries = parse_central_directory(&cd);
    if entries.is_empty() {
        f.evidence.push("zip:empty".into());
        return f;
    }

    if let Some(ct) = entries.iter().find(|e| e.name == CONTENT_TYPES) {
        match read_entry(source, ct, shift) {
            Some(xml) => classify_content_types(&xml, &mut f),
            None => f.evidence.push("zip:content-types-unreadable".into()),
        }
    }
    if f.format.is_none() {
        classify_by_part_names(entries.iter().map(|e| e.name.as_slice()), &mut f);
    }
    if entries.iter().any(|e| e.name.ends_with(b"vbaProject.bin")) {
        f.macro_enabled = true;
        f.evidence.push("zip:part:vbaProject.bin".into());
    }
    f
}

fn central_directory_location(
    source: &mut impl Source,
    tail: &[u8],
    eocd: usize,
    eocd_abs: u64,
) -> Option<(u64, u64, u64)> {
    let size32 = u32_at(tail, eocd + 12)?;
    let offset32 = u32_at(tail, eocd + 16)?;
    if size32 != u32::MAX && offset32 != u32::MAX {
        return Some((u64::from(offset32), u64::from(size32), eocd_abs));
    }
    // ZIP64: a 20-byte locator sits right before the EOCD record, and the
    // 56-byte ZIP64 record right before that. Locate the record by position,
    // not by the locator's stored offset, which prepended data would shift.
    let locator_at = eocd_abs.checked_sub(20)?;
    let locator = source.read_vec(locator_at, 20).ok()?;
    if &locator[..4] != ZIP64_LOCATOR_SIG {
        return None;
    }
    let record_at = locator_at.checked_sub(56)?;
    let record = source.read_vec(record_at, 56).ok()?;
    if &record[..4] != ZIP64_EOCD_SIG {
        return None;
    }
    Some((u64_at(&record, 48)?, u64_at(&record, 40)?, record_at))
}

fn parse_central_directory(cd: &[u8]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    while entries.len() < MAX_ENTRIES && cd.get(at..at + 4) == Some(CD_ENTRY_SIG) {
        let (Some(method), Some(csize), Some(usize_), Some(nlen), Some(xlen), Some(clen), Some(lo)) = (
            u16_at(cd, at + 10),
            u32_at(cd, at + 20),
            u32_at(cd, at + 24),
            u16_at(cd, at + 28),
            u16_at(cd, at + 30),
            u16_at(cd, at + 32),
            u32_at(cd, at + 42),
        ) else {
            break;
        };
        let name_start = at + 46;
        let name_end = name_start + usize::from(nlen);
        let Some(name) = cd.get(name_start..name_end) else {
            break;
        };
        let extra = cd
            .get(name_end..name_end + usize::from(xlen))
            .unwrap_or(&[]);
        let mut e = Entry {
            name: name.to_vec(),
            method,
            compressed_size: u64::from(csize),
            uncompressed_size: u64::from(usize_),
            local_offset: u64::from(lo),
        };
        apply_zip64_extra(extra, &mut e, usize_, csize, lo);
        entries.push(e);
        at = name_end + usize::from(xlen) + usize::from(clen);
    }
    entries
}

/// ZIP64 extended information (header 0x0001) holds only the fields that overflowed, in order.
fn apply_zip64_extra(extra: &[u8], e: &mut Entry, usize32: u32, csize32: u32, lo32: u32) {
    let mut at = 0;
    while let (Some(id), Some(len)) = (u16_at(extra, at), u16_at(extra, at + 2)) {
        let body = extra.get(at + 4..at + 4 + usize::from(len)).unwrap_or(&[]);
        if id == 1 {
            let mut p = 0;
            let mut next = || {
                let v = u64_at(body, p);
                p += 8;
                v
            };
            if usize32 == u32::MAX {
                e.uncompressed_size = next().unwrap_or(e.uncompressed_size);
            }
            if csize32 == u32::MAX {
                e.compressed_size = next().unwrap_or(e.compressed_size);
            }
            if lo32 == u32::MAX {
                e.local_offset = next().unwrap_or(e.local_offset);
            }
            return;
        }
        at += 4 + usize::from(len);
    }
}

/// Read one entry, inflating with a hard output limit. `None` on any problem.
fn read_entry(source: &mut impl Source, e: &Entry, shift: u64) -> Option<Vec<u8>> {
    let limit = MAX_CONTENT_TYPES_BYTES as u64;
    if e.compressed_size > limit || e.uncompressed_size > limit {
        return None;
    }
    let local_at = shift.checked_add(e.local_offset)?;
    let header = source.read_vec(local_at, 30).ok()?;
    if &header[..4] != LOCAL_SIG {
        return None;
    }
    let data_at = local_at + 30 + u64::from(u16_at(&header, 26)?) + u64::from(u16_at(&header, 28)?);
    let data = source.read_vec(data_at, e.compressed_size as usize).ok()?;
    match e.method {
        0 => Some(data),
        8 => {
            miniz_oxide::inflate::decompress_to_vec_with_limit(&data, MAX_CONTENT_TYPES_BYTES).ok()
        }
        _ => None,
    }
}

/// `(content type fragment, format, evidence label, macro-enabled)`. Fragments
/// are matched case-insensitively against `[Content_Types].xml`.
const MAIN_PARTS: &[(&str, Format, &str, bool)] = &[
    (
        "wordprocessingml.document.main+xml",
        Format::Docx,
        "wordprocessingml",
        false,
    ),
    (
        "wordprocessingml.template.main+xml",
        Format::Docx,
        "wordprocessingml-template",
        false,
    ),
    (
        "ms-word.document.macroenabled.main+xml",
        Format::Docx,
        "wordprocessingml-macro",
        true,
    ),
    (
        "ms-word.template.macroenabledtemplate.main+xml",
        Format::Docx,
        "wordprocessingml-macro",
        true,
    ),
    (
        "presentationml.presentation.main+xml",
        Format::Pptx,
        "presentationml",
        false,
    ),
    (
        "presentationml.slideshow.main+xml",
        Format::Pptx,
        "presentationml-slideshow",
        false,
    ),
    (
        "presentationml.template.main+xml",
        Format::Pptx,
        "presentationml-template",
        false,
    ),
    (
        "ms-powerpoint.presentation.macroenabled.main+xml",
        Format::Pptx,
        "presentationml-macro",
        true,
    ),
    (
        "ms-powerpoint.slideshow.macroenabled.main+xml",
        Format::Pptx,
        "presentationml-macro",
        true,
    ),
    (
        "ms-powerpoint.template.macroenabled.main+xml",
        Format::Pptx,
        "presentationml-macro",
        true,
    ),
    (
        "spreadsheetml.sheet.main+xml",
        Format::Xlsx,
        "spreadsheetml",
        false,
    ),
    (
        "spreadsheetml.template.main+xml",
        Format::Xlsx,
        "spreadsheetml-template",
        false,
    ),
    (
        "ms-excel.sheet.macroenabled.main+xml",
        Format::Xlsx,
        "spreadsheetml-macro",
        true,
    ),
    (
        "ms-excel.template.macroenabled.main+xml",
        Format::Xlsx,
        "spreadsheetml-macro",
        true,
    ),
];

fn classify_content_types(xml: &[u8], f: &mut ZipFinding) {
    let text = String::from_utf8_lossy(xml).to_ascii_lowercase();
    // Excel binary workbooks (.xlsb) are not XLSX.
    if text.contains("ms-excel.sheet.binary.macroenabled.main") {
        f.evidence.push("zip:content-types:xlsb".into());
        return;
    }
    let hit = MAIN_PARTS.iter().find(|(frag, ..)| text.contains(frag));
    match hit {
        Some(&(_, format, label, macro_enabled)) => {
            f.format = Some(format);
            f.confidence = Some(Confidence::Certain);
            f.macro_enabled |= macro_enabled;
            f.evidence.push(format!("zip:content-types:{label}"));
        }
        None => f.evidence.push("zip:content-types:no-main-part".into()),
    }
    if text.contains("vnd.ms-office.vbaproject") {
        f.macro_enabled = true;
    }
}

fn classify_by_part_names<'a>(names: impl Iterator<Item = &'a [u8]>, f: &mut ZipFinding) {
    const PARTS: &[(&[u8], Format)] = &[
        (b"word/document.xml", Format::Docx),
        (b"ppt/presentation.xml", Format::Pptx),
        (b"xl/workbook.xml", Format::Xlsx),
    ];
    for name in names {
        if let Some(&(part, format)) = PARTS.iter().find(|(p, _)| *p == name) {
            f.format = Some(format);
            f.confidence = Some(Confidence::Likely);
            f.evidence
                .push(format!("zip:part:{}", String::from_utf8_lossy(part)));
            return;
        }
    }
    if f.evidence
        .iter()
        .all(|e| !e.starts_with("zip:content-types"))
    {
        f.evidence.push("zip:no-office-parts".into());
    }
}

/// For a truncated archive with no central directory: is the first local
/// entry an OOXML part list?
pub fn first_local_entry_is_content_types(head: &[u8]) -> bool {
    if head.get(..4) != Some(LOCAL_SIG) {
        return false;
    }
    let Some(nlen) = u16_at(head, 26) else {
        return false;
    };
    head.get(30..30 + usize::from(nlen)) == Some(CONTENT_TYPES)
}
