//! Formats with no magic number: SVG, HTML, Markdown and plain text.
//! Only the head window is examined, and markup is never parsed.

use crate::{Confidence, Format};

pub struct TextFinding {
    pub format: Format,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
}

/// Decode the head as text, or `None` if it looks binary.
fn decode(head: &[u8], evidence: &mut Vec<String>) -> Option<String> {
    if let Some(rest) = head.strip_prefix(b"\xEF\xBB\xBF") {
        evidence.push("text:bom:utf-8".into());
        return decode_utf8(rest, evidence);
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units = rest.as_chunks::<2>().0.iter().map(|&c| {
            if le {
                u16::from_le_bytes(c)
            } else {
                u16::from_be_bytes(c)
            }
        });
        char::decode_utf16(units)
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
    };
    if let Some(rest) = head.strip_prefix(b"\xFF\xFE") {
        evidence.push("text:bom:utf-16le".into());
        return Some(utf16(rest, true)).filter(|s| !looks_binary(s));
    }
    if let Some(rest) = head.strip_prefix(b"\xFE\xFF") {
        evidence.push("text:bom:utf-16be".into());
        return Some(utf16(rest, false)).filter(|s| !looks_binary(s));
    }
    decode_utf8(head, evidence)
}

fn decode_utf8(bytes: &[u8], evidence: &mut Vec<String>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        // The window may cut a multi-byte character in half.
        Err(e) if e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        // Legacy 8-bit encodings: keep going if it's otherwise printable.
        Err(_) => {
            evidence.push("text:non-utf8".into());
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    Some(text).filter(|s| !looks_binary(s))
}

/// More than 1% control characters (other than whitespace) means binary.
fn looks_binary(s: &str) -> bool {
    let mut total = 0usize;
    let mut control = 0usize;
    for c in s.chars() {
        total += 1;
        if (c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c')) || c == '\0' {
            control += 1;
        }
    }
    total == 0 || control * 100 > total
}

/// Elements that, at the start of a document, mean HTML.
const HTML_START_TAGS: &[&str] = &[
    "meta", "title", "link", "style", "script", "div", "p", "h1", "h2", "h3", "table", "section",
    "article", "header", "nav", "main", "span", "ul", "ol", "form", "img", "a", "br",
];

pub fn classify(head: &[u8], claimed: Option<Format>) -> Option<TextFinding> {
    let mut evidence = Vec::new();
    let text = decode(head, &mut evidence)?;
    if let Some((format, confidence, ev)) = sniff_markup(&text) {
        evidence.push(ev);
        return Some(TextFinding {
            format,
            confidence,
            evidence,
        });
    }

    let score = markdown_score(&text);
    let (format, confidence, ev) = match claimed {
        Some(Format::Md) => (
            Format::Md,
            Confidence::Likely,
            "text:extension:md".to_owned(),
        ),
        Some(Format::Txt) => (
            Format::Txt,
            Confidence::Likely,
            "text:extension:txt".to_owned(),
        ),
        _ if score >= 3 => (
            Format::Md,
            Confidence::Guess,
            format!("text:markdown-score:{score}"),
        ),
        _ => (
            Format::Txt,
            Confidence::Guess,
            format!("text:markdown-score:{score}"),
        ),
    };
    evidence.push(ev);
    Some(TextFinding {
        format,
        confidence,
        evidence,
    })
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

/// Skip prologue (XML declaration, comments, doctype) and look at the root element.
fn sniff_markup(text: &str) -> Option<(Format, Confidence, String)> {
    let mut rest = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let mut xml_decl = false;
    loop {
        if rest.starts_with("<?") {
            xml_decl = true;
            rest = &rest[rest.find("?>")? + 2..];
        } else if rest.starts_with("<!--") {
            rest = &rest[rest.find("-->")? + 3..];
        } else if starts_with_ci(rest, "<!doctype") {
            let end = rest.find('>')?;
            let decl = rest[..end].to_ascii_lowercase();
            if decl.contains("svg") {
                return Some((Format::Svg, Confidence::Certain, "text:doctype:svg".into()));
            }
            if decl.contains("html") {
                return Some((
                    Format::Html,
                    Confidence::Certain,
                    "text:doctype:html".into(),
                ));
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
        rest = rest.trim_start();
    }

    if let Some(after) = rest.strip_prefix('<') {
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
            .collect::<String>()
            .to_ascii_lowercase();
        if name == "svg" || name.ends_with(":svg") {
            return Some((Format::Svg, Confidence::Certain, "text:root:svg".into()));
        }
        if matches!(name.as_str(), "html" | "head" | "body") {
            return Some((
                Format::Html,
                Confidence::Certain,
                format!("text:root:{name}"),
            ));
        }
        if HTML_START_TAGS.contains(&name.as_str()) {
            return Some((
                Format::Html,
                Confidence::Likely,
                format!("text:root:{name}"),
            ));
        }
        if xml_decl && !name.is_empty() {
            return Some((Format::Unknown, Confidence::Likely, "text:xml".into()));
        }
    }
    None
}

/// Count distinct kinds of Markdown syntax in the first lines.
fn markdown_score(text: &str) -> u32 {
    let mut heading = false;
    let mut list = false;
    let mut fence = false;
    let mut link = false;
    let mut emphasis = false;
    let mut quote = false;
    for line in text.lines().take(400) {
        let t = line.trim_start();
        let hashes = t.chars().take_while(|&c| c == '#').count();
        heading |= (1..=6).contains(&hashes) && t[hashes..].starts_with(' ');
        list |= t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ");
        list |= t
            .split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        fence |= t.starts_with("```") || t.starts_with("~~~");
        quote |= t.starts_with("> ");
        link |= t.contains("](") && t.contains('[');
        emphasis |= t.contains("**") || t.contains("__") || t.contains('`');
    }
    [heading, list, fence, link, emphasis, quote]
        .iter()
        .map(|&b| u32::from(b))
        .sum()
}
