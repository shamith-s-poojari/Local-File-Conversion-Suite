//! Spec section 6 test table: every format, renamed, truncated, polyglot,
//! non-ASCII and very long names, and rejections.

use converter_sniffer::{
    detect_bytes, sniff_bytes, sniff_path, Confidence, Detection, EncryptionHint, Format,
    SniffError,
};

// ---------------------------------------------------------------- builders

/// Minimal ZIP writer. `deflate` compresses entries with miniz_oxide.
fn zip(entries: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for (name, data) in entries {
        let (method, body) = if deflate {
            (8u16, miniz_oxide::deflate::compress_to_vec(data, 6))
        } else {
            (0u16, data.to_vec())
        };
        let offset = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0]);
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0; 8]); // time, date, crc (not checked)
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);

        cd.extend_from_slice(b"PK\x01\x02");
        cd.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
        cd.extend_from_slice(&method.to_le_bytes());
        cd.extend_from_slice(&[0; 8]);
        cd.extend_from_slice(&(body.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0; 12]);
        cd.extend_from_slice(&offset.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn content_types(main: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/main.xml" ContentType="{main}"/></Types>"#
    )
    .into_bytes()
}

const DOCX_CT: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const DOCM_CT: &str = "application/vnd.ms-word.document.macroEnabled.main+xml";
const PPTX_CT: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const PPSX_CT: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml";
const XLSX_CT: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const XLSM_CT: &str = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";
const XLSB_CT: &str = "application/vnd.ms-excel.sheet.binary.macroEnabled.main";

fn ooxml(ct: &str, main_part: &str, deflate: bool) -> Vec<u8> {
    zip(
        &[
            ("[Content_Types].xml", &content_types(ct)),
            ("_rels/.rels", b"<Relationships/>"),
            (main_part, b"<doc/>"),
        ],
        deflate,
    )
}

fn docx() -> Vec<u8> {
    ooxml(DOCX_CT, "word/document.xml", true)
}

fn pdf() -> Vec<u8> {
    b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n".to_vec()
}

fn big_pdf() -> Vec<u8> {
    // Larger than both windows, so head and tail are separate reads.
    let mut v = b"%PDF-1.4\n".to_vec();
    v.extend(std::iter::repeat_n(b'x', 200 * 1024));
    v.extend_from_slice(b"\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    v
}

fn png() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x10\0\0\0\x10\x08\x02\0\0\0".to_vec();
    v.extend_from_slice(&[0; 32]);
    v
}

fn jpg() -> Vec<u8> {
    let mut v = b"\xFF\xD8\xFF\xE0\0\x10JFIF\0\x01\x01".to_vec();
    v.extend_from_slice(&[0; 32]);
    v
}

fn webp() -> Vec<u8> {
    let mut v = b"RIFF\x24\0\0\0WEBPVP8 ".to_vec();
    v.extend_from_slice(&[0; 32]);
    v
}

fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
    let len = 16 + 4 * compat.len() as u32;
    let mut v = len.to_be_bytes().to_vec();
    v.extend_from_slice(b"ftyp");
    v.extend_from_slice(major);
    v.extend_from_slice(&[0; 4]);
    for b in compat {
        v.extend_from_slice(*b);
    }
    v.extend_from_slice(b"\0\0\0\x08meta");
    v
}

fn heic() -> Vec<u8> {
    ftyp(b"heic", &[b"mif1", b"heic"])
}

fn cfb(extra: &[u8]) -> Vec<u8> {
    let mut v = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1".to_vec();
    v.extend_from_slice(&[0; 504]);
    v.extend_from_slice(extra);
    v
}

fn utf16le(s: &str, bom: bool) -> Vec<u8> {
    let mut v = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
    v.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
    v
}

fn utf16be(s: &str) -> Vec<u8> {
    let mut v = vec![0xFE, 0xFF];
    v.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
    v
}

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect/></svg>"#;
const HTML: &str =
    "<!DOCTYPE html>\n<html><head><title>x</title></head><body><p>Hi</p></body></html>";
const MD: &str = "# Title\n\nSome *text* with a [link](http://example.invalid).\n\n- one\n- two\n\n```\ncode\n```\n";
const TXT: &str = "Meeting notes\nBring the report.\nCall back on Monday.\n";

// ------------------------------------------------------------------- table

struct Case {
    id: &'static str,
    name: String,
    bytes: Vec<u8>,
    format: Format,
    confidence: Option<Confidence>,
    mismatch: bool,
    candidates: &'static [Format],
    encryption: EncryptionHint,
    macro_enabled: bool,
    evidence: &'static [&'static str],
}

fn case(id: &'static str, name: impl Into<String>, bytes: Vec<u8>, format: Format) -> Case {
    Case {
        id,
        name: name.into(),
        bytes,
        format,
        confidence: None,
        mismatch: false,
        candidates: &[],
        encryption: EncryptionHint::None,
        macro_enabled: false,
        evidence: &[],
    }
}

impl Case {
    fn conf(mut self, c: Confidence) -> Self {
        self.confidence = Some(c);
        self
    }
    fn mismatch(mut self) -> Self {
        self.mismatch = true;
        self
    }
    fn candidates(mut self, c: &'static [Format]) -> Self {
        self.candidates = c;
        self
    }
    fn encrypted(mut self) -> Self {
        self.encryption = EncryptionHint::Maybe;
        self
    }
    fn macros(mut self) -> Self {
        self.macro_enabled = true;
        self
    }
    fn evidence(mut self, e: &'static [&'static str]) -> Self {
        self.evidence = e;
        self
    }
}

fn cases() -> Vec<Case> {
    use Confidence::*;
    use Format::*;

    let mut pdf_with_junk = b"garbage header bytes\r\n".to_vec();
    pdf_with_junk.extend(pdf());

    let mut truncated_pdf = pdf();
    truncated_pdf.truncate(30);

    let mut big_truncated_pdf = big_pdf();
    big_truncated_pdf.truncate(150 * 1024);

    let mut encrypted_pdf = pdf();
    encrypted_pdf.splice(70..70, b"/Encrypt 5 0 R ".iter().copied());

    let mut pdf_zip = pdf();
    pdf_zip.extend(docx());

    let mut png_pdf = png();
    png_pdf.extend_from_slice(b"tEXt%PDF-1.4 hidden");

    let mut truncated_docx = docx();
    truncated_docx.truncate(60);

    // A 100 KB stored entry first puts the central directory far from the head.
    let filler = vec![b'a'; 100 * 1024];
    let big_docx_tail = zip(
        &[
            ("word/media/filler.bin", &filler),
            ("[Content_Types].xml", &content_types(DOCX_CT)),
            ("word/document.xml", b"<doc/>"),
        ],
        false,
    );

    let mut docx_with_comment = docx();
    let n = docx_with_comment.len();
    docx_with_comment[n - 2..].copy_from_slice(&7u16.to_le_bytes());
    docx_with_comment.extend_from_slice(b"comment");

    let mut text_cut_mid_char = "Grüße aus Köln. ".repeat(5000).into_bytes();
    text_cut_mid_char.truncate(64 * 1024 + 1);

    let mut binary = Vec::new();
    for i in 0..4096u32 {
        binary.push((i.wrapping_mul(2_654_435_761) >> 13) as u8);
    }

    let long_name = format!("{}.pdf", "a".repeat(300));

    vec![
        // PDF
        case("pdf/basic", "a.pdf", pdf(), Pdf)
            .conf(Certain)
            .evidence(&["magic:%PDF"]),
        case("pdf/large", "large.pdf", big_pdf(), Pdf).conf(Certain),
        case("pdf/no-extension", "scan", pdf(), Pdf).conf(Certain),
        case("pdf/uppercase-ext", "REPORT.PDF", pdf(), Pdf).conf(Certain),
        case("pdf/leading-junk", "a.pdf", pdf_with_junk, Pdf).conf(Likely),
        case("pdf/truncated-no-eof", "a.pdf", truncated_pdf, Pdf).evidence(&["pdf:no-eof"]),
        case("pdf/large-truncated", "a.pdf", big_truncated_pdf, Pdf).evidence(&["pdf:no-eof"]),
        case("pdf/encrypt-marker", "a.pdf", encrypted_pdf, Pdf).encrypted(),
        case("pdf/renamed-docx", "a.docx", pdf(), Pdf).mismatch(),
        case("pdf/renamed-txt", "a.txt", pdf(), Pdf).mismatch(),
        case(
            "pdf/magic-after-1k",
            "a.pdf",
            [vec![b' '; 2000], pdf()].concat(),
            Txt,
        )
        .mismatch(),
        case("pdf/non-ascii-name", "Отчёт 报告 ✓.pdf", pdf(), Pdf),
        case("pdf/very-long-name", long_name, pdf(), Pdf),
        case("pdf/name-with-dots", "v1.2.final.pdf", pdf(), Pdf),
        case("pdf/hidden-name", ".pdf", pdf(), Pdf),
        // OOXML
        case("docx/deflated", "a.docx", docx(), Docx)
            .conf(Certain)
            .evidence(&["zip:content-types:wordprocessingml"]),
        case(
            "docx/stored",
            "a.docx",
            ooxml(DOCX_CT, "word/document.xml", false),
            Docx,
        )
        .conf(Certain),
        case("docx/no-extension", "a", docx(), Docx).conf(Certain),
        case("docx/renamed-pdf", "a.pdf", docx(), Docx).mismatch(),
        case("docx/renamed-zip", "a.zip", docx(), Docx),
        case(
            "docx/docm",
            "a.docm",
            ooxml(DOCM_CT, "word/document.xml", true),
            Docx,
        )
        .macros(),
        case(
            "docx/docm-renamed",
            "a.docx",
            ooxml(DOCM_CT, "word/document.xml", true),
            Docx,
        )
        .macros(),
        case(
            "docx/vba-part",
            "a.docx",
            zip(
                &[
                    ("[Content_Types].xml", &content_types(DOCX_CT)),
                    ("word/vbaProject.bin", b"x"),
                ],
                true,
            ),
            Docx,
        )
        .macros(),
        case("docx/cd-far-from-head", "a.docx", big_docx_tail, Docx).conf(Certain),
        case("docx/zip-comment", "a.docx", docx_with_comment, Docx).conf(Certain),
        case(
            "docx/no-content-types",
            "a.docx",
            zip(&[("word/document.xml", b"<d/>")], true),
            Docx,
        )
        .conf(Likely),
        case("docx/truncated", "a.docx", truncated_docx.clone(), Docx).conf(Guess),
        case("docx/truncated-no-ext", "a", truncated_docx, Unknown)
            .evidence(&["zip:no-central-directory"]),
        case(
            "pptx/basic",
            "a.pptx",
            ooxml(PPTX_CT, "ppt/presentation.xml", true),
            Pptx,
        )
        .conf(Certain),
        case(
            "pptx/slideshow",
            "a.ppsx",
            ooxml(PPSX_CT, "ppt/presentation.xml", true),
            Pptx,
        ),
        case(
            "pptx/renamed-docx",
            "a.docx",
            ooxml(PPTX_CT, "ppt/presentation.xml", true),
            Pptx,
        )
        .mismatch(),
        case(
            "xlsx/basic",
            "a.xlsx",
            ooxml(XLSX_CT, "xl/workbook.xml", true),
            Xlsx,
        )
        .conf(Certain),
        case(
            "xlsx/macro",
            "a.xlsm",
            ooxml(XLSM_CT, "xl/workbook.xml", true),
            Xlsx,
        )
        .macros(),
        case(
            "xlsx/xlsb-not-xlsx",
            "a.xlsx",
            ooxml(XLSB_CT, "xl/workbook.bin", true),
            Unknown,
        )
        .mismatch(),
        case(
            "zip/plain",
            "a.zip",
            zip(&[("readme.txt", b"hi")], true),
            Unknown,
        )
        .evidence(&["magic:zip", "zip:no-office-parts"]),
        case(
            "zip/plain-as-docx",
            "a.docx",
            zip(&[("readme.txt", b"hi")], false),
            Unknown,
        )
        .mismatch(),
        case(
            "zip/epub",
            "a.epub",
            zip(&[("mimetype", b"application/epub+zip")], false),
            Unknown,
        ),
        case("zip/empty-archive", "a.zip", zip(&[], false), Unknown),
        // Legacy / encrypted Office containers
        case("cfb/legacy-doc", "a.doc", cfb(b""), Unknown).evidence(&["magic:cfb"]),
        case(
            "cfb/encrypted-docx",
            "a.docx",
            cfb(&utf16le("EncryptionInfo", false)),
            Docx,
        )
        .conf(Likely)
        .encrypted(),
        case(
            "cfb/encrypted-xlsx",
            "a.xlsx",
            cfb(&utf16le("EncryptionInfo", false)),
            Xlsx,
        )
        .encrypted(),
        case(
            "cfb/encrypted-no-ext",
            "a",
            cfb(&utf16le("EncryptionInfo", false)),
            Unknown,
        )
        .encrypted(),
        // Images
        case("png/basic", "a.png", png(), Png).conf(Certain),
        case("png/renamed-jpg", "a.jpg", png(), Png).mismatch(),
        case(
            "png/truncated-magic-only",
            "a.png",
            png()[..8].to_vec(),
            Png,
        ),
        case("jpg/basic", "a.jpg", jpg(), Jpg).conf(Certain),
        case("jpg/jpeg-ext", "a.jpeg", jpg(), Jpg),
        case("jpg/renamed-png", "photo.png", jpg(), Jpg).mismatch(),
        case("webp/basic", "a.webp", webp(), Webp).conf(Certain),
        case(
            "webp/riff-not-webp",
            "a.wav",
            b"RIFF\x24\0\0\0WAVEfmt ".to_vec(),
            Unknown,
        ),
        case("heic/basic", "a.heic", heic(), Heic).conf(Certain),
        case(
            "heic/mif1-generic",
            "a.heif",
            ftyp(b"mif1", &[b"mif1"]),
            Heic,
        )
        .conf(Likely),
        case(
            "heic/avif-not-heic",
            "a.avif",
            ftyp(b"mif1", &[b"avif", b"mif1"]),
            Unknown,
        ),
        case(
            "heic/mp4-not-heic",
            "a.mp4",
            ftyp(b"isom", &[b"mp41"]),
            Unknown,
        ),
        case("heic/renamed-jpg", "IMG_0001.jpg", heic(), Heic).mismatch(),
        // Text
        case("svg/basic", "a.svg", SVG.into(), Svg).conf(Certain),
        case(
            "svg/xml-decl-comment",
            "a.svg",
            format!("<?xml version=\"1.0\"?>\n<!-- made by hand -->\n{SVG}").into_bytes(),
            Svg,
        ),
        case(
            "svg/doctype",
            "a.svg",
            format!("<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\">{SVG}").into_bytes(),
            Svg,
        ),
        case(
            "svg/namespaced-root",
            "a.svg",
            b"<svg:svg xmlns:svg=\"x\"></svg:svg>".to_vec(),
            Svg,
        ),
        case("svg/renamed-txt", "a.txt", SVG.into(), Svg).mismatch(),
        case(
            "svg/utf8-bom",
            "a.svg",
            [b"\xEF\xBB\xBF".as_slice(), SVG.as_bytes()].concat(),
            Svg,
        )
        .evidence(&["text:bom:utf-8"]),
        case("html/doctype", "a.html", HTML.into(), Html).conf(Certain),
        case("html/htm-ext", "a.htm", HTML.into(), Html),
        case(
            "html/no-doctype",
            "a.html",
            b"<html><body>x</body></html>".to_vec(),
            Html,
        )
        .conf(Certain),
        case(
            "html/fragment",
            "a.html",
            b"<div class=\"x\">hello</div>".to_vec(),
            Html,
        )
        .conf(Likely),
        case("html/utf16-bom", "a.html", utf16le(HTML, true), Html)
            .evidence(&["text:bom:utf-16le"]),
        case("html/utf16be-bom", "a.html", utf16be(HTML), Html).evidence(&["text:bom:utf-16be"]),
        case(
            "xml/generic",
            "a.xml",
            b"<?xml version=\"1.0\"?><note>hi</note>".to_vec(),
            Unknown,
        )
        .evidence(&["text:xml"]),
        case("md/by-extension", "a.md", MD.into(), Md).conf(Likely),
        case("md/by-content", "README", MD.into(), Md).conf(Guess),
        case("md/as-txt", "a.txt", MD.into(), Txt).conf(Likely),
        case("txt/basic", "a.txt", TXT.into(), Txt).conf(Likely),
        case("txt/no-ext", "notes", TXT.into(), Txt).conf(Guess),
        case("txt/md-ext", "notes.md", TXT.into(), Md),
        case(
            "txt/latin1",
            "a.txt",
            b"Caf\xe9 cr\xe8me br\xfbl\xe9e\n".to_vec(),
            Txt,
        )
        .evidence(&["text:non-utf8"]),
        case("txt/cut-mid-char", "a.txt", text_cut_mid_char, Txt),
        case("txt/renamed-pdf", "a.pdf", TXT.into(), Txt).mismatch(),
        case("txt/single-byte", "a.txt", b"x".to_vec(), Txt),
        // Binary / unknown
        case("binary/random", "a.bin", binary.clone(), Unknown).evidence(&["binary:unrecognized"]),
        case("binary/as-pdf", "a.pdf", binary, Unknown).mismatch(),
        case(
            "binary/nul-bytes",
            "a.txt",
            b"abc\0\0\0def".to_vec(),
            Unknown,
        )
        .mismatch(),
        case(
            "binary/exe",
            "a.exe",
            b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff".to_vec(),
            Unknown,
        ),
        // Polyglots
        case("polyglot/pdf-zip", "a.pdf", pdf_zip.clone(), Pdf)
            .candidates(&[Pdf, Docx])
            .evidence(&["polyglot"]),
        case("polyglot/pdf-zip-as-docx", "a.docx", pdf_zip, Pdf).candidates(&[Pdf, Docx]),
        case("polyglot/png-pdf", "a.png", png_pdf.clone(), Png).candidates(&[Png, Pdf]),
        case("polyglot/png-pdf-as-txt", "a.txt", png_pdf, Png)
            .candidates(&[Png, Pdf])
            .mismatch(),
    ]
}

fn check(c: &Case, d: &Detection) {
    let id = c.id;
    assert_eq!(
        d.format, c.format,
        "{id}: format; evidence {:?}",
        d.evidence
    );
    if let Some(conf) = c.confidence {
        assert_eq!(d.confidence, conf, "{id}: confidence");
    }
    assert_eq!(
        d.extension_mismatch, c.mismatch,
        "{id}: mismatch; evidence {:?}",
        d.evidence
    );
    assert_eq!(d.candidates, c.candidates, "{id}: candidates");
    assert_eq!(d.encryption_hint, c.encryption, "{id}: encryption hint");
    assert_eq!(d.macro_enabled, c.macro_enabled, "{id}: macro");
    for e in c.evidence {
        assert!(
            d.evidence.iter().any(|x| x == e),
            "{id}: missing evidence {e:?} in {:?}",
            d.evidence
        );
    }
}

#[test]
fn table_has_at_least_60_cases() {
    assert!(
        cases().len() >= 60,
        "spec section 6 requires at least 60 cases"
    );
}

#[test]
fn table() {
    let mut failures = Vec::new();
    for c in cases() {
        let result = std::panic::catch_unwind(|| {
            let d = detect_bytes(&c.name, &c.bytes).expect("detect");
            check(&c, &d);
        });
        if let Err(e) = result {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failures.push(msg);
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_planned_format_is_covered() {
    use Format::*;
    let covered: Vec<Format> = cases().iter().map(|c| c.format).collect();
    for f in [
        Pdf, Docx, Pptx, Xlsx, Md, Html, Txt, Png, Jpg, Webp, Svg, Heic, Unknown,
    ] {
        assert!(covered.contains(&f), "no case for {f:?}");
    }
}

// --------------------------------------------------------- descriptor rules

#[test]
fn only_unambiguous_pdf_is_actionable() {
    assert!(sniff_bytes("a.pdf", &pdf()).unwrap().actionable);
    assert!(!sniff_bytes("a.docx", &docx()).unwrap().actionable);
    let mut polyglot = pdf();
    polyglot.extend(docx());
    assert!(!sniff_bytes("a.pdf", &polyglot).unwrap().actionable);
}

#[test]
fn file_ids_are_random_and_not_derived_from_the_name() {
    let a = sniff_bytes("same.pdf", &pdf()).unwrap();
    let b = sniff_bytes("same.pdf", &pdf()).unwrap();
    assert_ne!(a.file_id, b.file_id);
    assert!(!a.file_id.contains("same"));
    assert_eq!(a.file_id.len(), 34);
}

#[test]
fn descriptor_serializes_in_camel_case() {
    let d = sniff_bytes("a.pdf", &pdf()).unwrap();
    let v = serde_json::to_value(&d).unwrap();
    for key in [
        "fileId",
        "displayName",
        "sizeBytes",
        "format",
        "confidence",
        "evidence",
        "extensionMismatch",
        "candidates",
        "encryptionHint",
        "macroEnabled",
        "cloudPlaceholder",
        "actionable",
    ] {
        assert!(v.get(key).is_some(), "missing {key}");
    }
    assert_eq!(v["format"], "pdf");
    assert_eq!(v["confidence"], "certain");
    assert_eq!(v["encryptionHint"], "none");
}

#[test]
fn display_name_is_kept_verbatim() {
    let name = "<img src=x onerror=alert(1)>.pdf";
    assert_eq!(sniff_bytes(name, &pdf()).unwrap().display_name, name);
}

// ---------------------------------------------------------------- rejections

#[test]
fn zero_bytes_is_rejected() {
    assert_eq!(detect_bytes("a.pdf", b""), Err(SniffError::Empty));
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sniffer-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn path_rejections() {
    let dir = temp_dir("reject");
    assert_eq!(sniff_path(&dir), Err(SniffError::Directory));
    assert_eq!(
        sniff_path(&dir.join("missing.pdf")),
        Err(SniffError::NotFound)
    );
    let empty = dir.join("empty.pdf");
    std::fs::write(&empty, b"").unwrap();
    assert_eq!(sniff_path(&empty), Err(SniffError::Empty));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn path_reads_real_files() {
    let dir = temp_dir("read");
    let p = dir.join("Résumé – final.pdf");
    std::fs::write(&p, big_pdf()).unwrap();
    let d = sniff_path(&p).unwrap();
    assert_eq!(d.format, Format::Pdf);
    assert_eq!(d.display_name, "Résumé – final.pdf");
    assert_eq!(d.size_bytes, big_pdf().len() as u64);
    assert!(!d.cloud_placeholder);

    let p = dir.join("deck.pptx");
    std::fs::write(&p, ooxml(PPTX_CT, "ppt/presentation.xml", true)).unwrap();
    assert_eq!(sniff_path(&p).unwrap().format, Format::Pptx);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn special_files_are_rejected() {
    assert_eq!(
        sniff_path(std::path::Path::new("/dev/null")),
        Err(SniffError::SpecialFile)
    );
}

// ------------------------------------------------------------------ no panic

/// Cheap in-CI stand-in for the nightly fuzz target.
#[test]
fn random_and_mutated_inputs_never_panic() {
    let seeds = [
        pdf(),
        docx(),
        png(),
        heic(),
        cfb(b""),
        SVG.as_bytes().to_vec(),
        webp(),
    ];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for i in 0..3000 {
        let mut bytes = seeds[i % seeds.len()].clone();
        let len = bytes.len();
        for _ in 0..(next() % 8) {
            let at = (next() as usize) % len;
            bytes[at] = next() as u8;
        }
        bytes.truncate(1 + (next() as usize) % len);
        let _ = detect_bytes("x.bin", &bytes);
        let random: Vec<u8> = (0..(next() % 512)).map(|_| next() as u8).collect();
        let _ = detect_bytes("y", &random);
    }
}
