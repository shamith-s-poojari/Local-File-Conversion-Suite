//! Binary formats identified by a signature near the start of the file:
//! more images, audio and video containers, and legacy Office files.
//!
//! Every check reads only the head window (and, for legacy Office, one
//! directory sector). Nothing is decoded.

use crate::source::Source;
use crate::{Confidence, Format};

/// A signature match.
pub struct Magic {
    pub format: Format,
    pub confidence: Confidence,
    pub evidence: String,
}

fn hit(format: Format, confidence: Confidence, evidence: impl Into<String>) -> Option<Magic> {
    Some(Magic {
        format,
        confidence,
        evidence: evidence.into(),
    })
}

fn u16_le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u32_be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Images other than PNG, JPEG, WebP and HEIC.
pub fn image(head: &[u8]) -> Option<Magic> {
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return hit(Format::Gif, Confidence::Certain, "magic:gif");
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") || head.starts_with(b"II+\0") {
        return hit(Format::Tiff, Confidence::Certain, "magic:tiff");
    }
    if head.starts_with(b"BM") {
        // The DIB header size identifies a real bitmap rather than text starting "BM".
        if let Some(dib) = u32_le(head, 14) {
            if matches!(dib, 12 | 40 | 52 | 56 | 64 | 108 | 124) && u32_le(head, 6) == Some(0) {
                return hit(Format::Bmp, Confidence::Certain, "magic:bmp");
            }
        }
    }
    if head.starts_with(b"\0\0\x01\0") {
        return ico(head);
    }
    None
}

/// ICO has a weak signature, so every directory entry must be plausible.
fn ico(head: &[u8]) -> Option<Magic> {
    let count = usize::from(u16_le(head, 4)?);
    if count == 0 || count > 256 {
        return None;
    }
    let table_end = 6 + 16 * count;
    for i in 0..count.min(16) {
        let e = head.get(6 + 16 * i..6 + 16 * (i + 1))?;
        let planes = u16_le(e, 4)?;
        let size = u32_le(e, 8)?;
        let offset = u32_le(e, 12)? as usize;
        if e[3] != 0 || planes > 1 || size == 0 || offset < table_end {
            return None;
        }
    }
    hit(Format::Ico, Confidence::Likely, "magic:ico")
}

/// ISO BMFF (`ftyp`) brands that aren't HEIF images.
pub fn bmff(head: &[u8]) -> Option<Magic> {
    if head.len() >= 16 && &head[4..8] == b"ftyp" {
        let major = &head[8..12];
        let box_len = u32_be(head, 0)? as usize;
        let compat_end = box_len.clamp(16, head.len());
        let compat: Vec<&[u8]> = head
            .get(16..compat_end)
            .unwrap_or(&[])
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| c.as_slice())
            .collect();
        let brand = String::from_utf8_lossy(major).trim_end().to_owned();
        let ev = format!("magic:ftyp:{brand}");
        if matches!(major, b"avif" | b"avis")
            || (matches!(major, b"mif1" | b"msf1")
                && compat.iter().any(|b| *b == b"avif" || *b == b"avis"))
        {
            return hit(Format::Avif, Confidence::Certain, ev);
        }
        if matches!(major, b"M4A " | b"M4B " | b"M4P " | b"F4A " | b"F4B ") {
            return hit(Format::M4a, Confidence::Certain, ev);
        }
        if major == b"qt  " {
            return hit(Format::Mov, Confidence::Certain, ev);
        }
        // HEIF image brands are the HEIC check's business.
        if matches!(
            major,
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1"
        ) {
            return None;
        }
        return hit(Format::Mp4, Confidence::Certain, ev);
    }
    // QuickTime files from before `ftyp` start with a top-level atom.
    if head.len() >= 8 && matches!(&head[4..8], b"moov" | b"mdat" | b"wide" | b"free" | b"pnot") {
        let len = u32_be(head, 0)?;
        if len >= 8 {
            return hit(Format::Mov, Confidence::Likely, "magic:quicktime-atom");
        }
    }
    None
}

/// Audio and video containers.
pub fn media(head: &[u8]) -> Option<Magic> {
    if head.len() >= 12 && (&head[..4] == b"RIFF" || &head[..4] == b"RF64") {
        match &head[8..12] {
            b"WAVE" => return hit(Format::Wav, Confidence::Certain, "magic:riff-wave"),
            b"AVI " => return hit(Format::Avi, Confidence::Certain, "magic:riff-avi"),
            _ => {}
        }
    }
    if head.len() >= 12 && &head[..4] == b"FORM" && matches!(&head[8..12], b"AIFF" | b"AIFC") {
        return hit(Format::Aiff, Confidence::Certain, "magic:form-aiff");
    }
    if head.starts_with(b"fLaC") {
        return hit(Format::Flac, Confidence::Certain, "magic:flac");
    }
    if head.starts_with(b"OggS") {
        return ogg(head);
    }
    if head.starts_with(b"\x1A\x45\xDF\xA3") {
        return ebml(head);
    }
    if head.starts_with(b"ID3") {
        return after_id3(head);
    }
    mpeg_audio(head, 0, "magic:mpeg-audio")
}

/// The codec in an Ogg stream's first page decides Vorbis (`ogg`) or Opus.
fn ogg(head: &[u8]) -> Option<Magic> {
    let segments = usize::from(*head.get(26)?);
    let payload = head.get(27 + segments..)?;
    if payload.starts_with(b"OpusHead") {
        return hit(Format::Opus, Confidence::Certain, "magic:ogg:opus");
    }
    if payload.starts_with(b"\x01vorbis")
        || payload.starts_with(b"\x7fFLAC")
        || payload.starts_with(b"Speex")
    {
        return hit(Format::Ogg, Confidence::Certain, "magic:ogg:audio");
    }
    // Theora and other video in Ogg isn't supported.
    None
}

/// Matroska and WebM share EBML; the DocType element tells them apart.
fn ebml(head: &[u8]) -> Option<Magic> {
    let window = &head[..head.len().min(128)];
    let at = window.windows(2).position(|w| w == [0x42, 0x82])?;
    let size = usize::from(*window.get(at + 2)? & 0x7F);
    let doctype = window.get(at + 3..at + 3 + size)?;
    match doctype {
        b"webm" => hit(Format::Webm, Confidence::Certain, "magic:ebml:webm"),
        b"matroska" => hit(Format::Mkv, Confidence::Certain, "magic:ebml:matroska"),
        _ => None,
    }
}

/// An ID3v2 tag precedes MP3 frames, but also some FLAC and AAC files.
fn after_id3(head: &[u8]) -> Option<Magic> {
    let b = head.get(6..10)?;
    if b.iter().any(|x| x & 0x80 != 0) {
        return None;
    }
    let size = (usize::from(b[0]) << 21)
        | (usize::from(b[1]) << 14)
        | (usize::from(b[2]) << 7)
        | usize::from(b[3]);
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    let start = 10 + size + footer;
    match head.get(start..) {
        Some(rest) if rest.starts_with(b"fLaC") => {
            hit(Format::Flac, Confidence::Certain, "magic:id3+flac")
        }
        Some(rest) if adts_len(rest).is_some() => {
            hit(Format::Aac, Confidence::Certain, "magic:id3+adts")
        }
        Some(_) => mpeg_audio(head, start, "magic:id3+mpeg-audio")
            .or_else(|| hit(Format::Mp3, Confidence::Likely, "magic:id3")),
        // The tag is larger than the window (cover art): MP3 is by far the usual case.
        None => hit(Format::Mp3, Confidence::Likely, "magic:id3"),
    }
}

/// Two consecutive MPEG audio frames (Layer III is MP3), or ADTS AAC.
fn mpeg_audio(head: &[u8], at: usize, evidence: &str) -> Option<Magic> {
    let rest = head.get(at..)?;
    if let Some(len) = adts_len(rest) {
        let confidence = match rest.get(len..) {
            Some(next) if adts_len(next).is_some() => Confidence::Certain,
            Some([]) => Confidence::Likely,
            _ => return None,
        };
        return hit(Format::Aac, confidence, "magic:adts");
    }
    let (len, layer3) = mpeg_frame_len(rest)?;
    if !layer3 {
        return None;
    }
    let confidence = match rest.get(len..) {
        Some(next) if mpeg_frame_len(next).is_some() => Confidence::Certain,
        // One frame that ends exactly at the end of the window or file.
        Some(next) if next.len() < 4 => Confidence::Likely,
        _ => return None,
    };
    hit(Format::Mp3, confidence, evidence)
}

fn adts_len(h: &[u8]) -> Option<usize> {
    if h.len() < 7 || h[0] != 0xFF || h[1] & 0xF6 != 0xF0 {
        return None;
    }
    // Sampling frequency index 13 to 15 is reserved.
    if (h[2] >> 2) & 0x0F > 12 {
        return None;
    }
    let len = (usize::from(h[3] & 0x03) << 11) | (usize::from(h[4]) << 3) | usize::from(h[5] >> 5);
    (len >= 7).then_some(len)
}

/// Frame length and whether it is Layer III.
fn mpeg_frame_len(h: &[u8]) -> Option<(usize, bool)> {
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0: 2.5, 1: reserved, 2: v2, 3: v1
    let layer = (h[1] >> 1) & 3; // 1: III, 2: II, 3: I
    let bitrate_index = usize::from(h[2] >> 4);
    let rate_index = usize::from((h[2] >> 2) & 3);
    let padding = usize::from((h[2] >> 1) & 1);
    if version == 1 || layer == 0 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return None;
    }
    const V1: [[u32; 15]; 3] = [
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ],
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ],
        [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ],
    ];
    const V2: [[u32; 15]; 2] = [
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ],
    ];
    let rates: [u32; 3] = match version {
        3 => [44100, 48000, 32000],
        2 => [22050, 24000, 16000],
        _ => [11025, 12000, 8000],
    };
    let rate = rates[rate_index];
    let kbps = if version == 3 {
        V1[usize::from(layer - 1)][bitrate_index]
    } else if layer == 3 {
        V2[1][bitrate_index]
    } else {
        V2[0][bitrate_index]
    };
    let bits = kbps * 1000;
    let len = match layer {
        3 => (12 * bits / rate + padding as u32) * 4,
        1 if version != 3 => 72 * bits / rate + padding as u32,
        _ => 144 * bits / rate + padding as u32,
    } as usize;
    (len >= 4).then_some((len, layer == 1))
}

/// Legacy Office (Compound File Binary): the stream names say which.
/// Names are UTF-16LE in directory entries; the first directory sector is
/// read from the header, and the head and tail windows are searched too.
pub fn legacy_office(source: &mut impl Source, head: &[u8], tail: &[u8]) -> Option<Magic> {
    let mut haystacks: Vec<Vec<u8>> = vec![head.to_vec(), tail.to_vec()];
    if let (Some(shift), Some(first)) = (u16_le(head, 0x1E), u32_le(head, 0x30)) {
        if matches!(shift, 9 | 12) && first < 0xFFFF_FFFA {
            let size = 1u64 << shift;
            let at = (u64::from(first) + 1) * size;
            if at + size <= source.len() {
                if let Ok(sector) = source.read_vec(at, size as usize) {
                    haystacks.push(sector);
                }
            }
        }
    }
    let has = |name: &str| {
        let needle = utf16le(name);
        haystacks
            .iter()
            .any(|h| h.windows(needle.len()).any(|w| w == needle))
    };
    if has("WordDocument") {
        return hit(Format::Doc, Confidence::Certain, "cfb:stream:WordDocument");
    }
    if has("Workbook") || has("Book") {
        return hit(Format::Xls, Confidence::Certain, "cfb:stream:Workbook");
    }
    if has("PowerPoint Document") {
        return hit(Format::Ppt, Confidence::Certain, "cfb:stream:PowerPoint");
    }
    None
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp3_frame() -> Vec<u8> {
        // MPEG-1 Layer III, 128 kbps, 44.1 kHz, no padding: 417 bytes.
        let mut f = vec![0xFF, 0xFB, 0x90, 0x00];
        f.resize(417, 0);
        f
    }

    #[test]
    fn mp3_needs_two_frames() {
        let mut two = mp3_frame();
        two.extend(mp3_frame());
        two.extend([0u8; 8]);
        assert_eq!(media(&two).unwrap().format, Format::Mp3);
        let mut junk = mp3_frame();
        junk.extend([0x12u8; 32]);
        assert!(media(&junk).is_none());
    }

    #[test]
    fn jpeg_is_not_mpeg_audio() {
        assert!(media(b"\xFF\xD8\xFF\xE0\0\x10JFIF\0").is_none());
    }

    #[test]
    fn ebml_doctype() {
        let mut h = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01];
        h.extend([0x42, 0x82, 0x84]);
        h.extend(b"webm");
        assert_eq!(media(&h).unwrap().format, Format::Webm);
    }

    #[test]
    fn ftyp_brands() {
        let ftyp = |brand: &[u8; 4]| {
            let mut h = vec![0, 0, 0, 20];
            h.extend(b"ftyp");
            h.extend(brand);
            h.extend([0, 0, 0, 0]);
            h.extend(b"isom");
            h
        };
        assert_eq!(bmff(&ftyp(b"M4A ")).unwrap().format, Format::M4a);
        assert_eq!(bmff(&ftyp(b"qt  ")).unwrap().format, Format::Mov);
        assert_eq!(bmff(&ftyp(b"isom")).unwrap().format, Format::Mp4);
        assert_eq!(bmff(&ftyp(b"avif")).unwrap().format, Format::Avif);
        assert!(bmff(&ftyp(b"heic")).is_none());
    }

    #[test]
    fn text_starting_bm_is_not_a_bitmap() {
        assert!(image(b"BM is a set of initials, not a bitmap header").is_none());
    }
}
