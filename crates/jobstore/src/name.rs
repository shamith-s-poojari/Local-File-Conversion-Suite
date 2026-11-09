//! Sanitizing worker-suggested file names (spec section 7.4).

/// Longest name we produce, in bytes. Every OS we target allows 255; leave
/// room for a collision suffix such as ` (12)`.
const MAX_NAME_BYTES: usize = 200;

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turns a worker's `suggestedName` into a safe file name ending in
/// `.{extension}`.
///
/// - Keeps only the last path component, so `../../x` can't escape.
/// - Drops control characters, characters Windows forbids, and invisible
///   formatting characters such as right-to-left overrides that could
///   disguise the real extension.
/// - Trims leading and trailing dots and spaces, avoids Windows device names
///   such as `CON`, and limits the length.
/// - Forces the extension of the declared output format.
///
/// Falls back to `fallback_stem` when nothing usable is left. An invalid
/// `extension` (anything but 1 to 10 ASCII letters and digits) is ignored.
pub fn sanitize_file_name(suggested: &str, extension: &str, fallback_stem: &str) -> String {
    let ext = extension.to_ascii_lowercase();
    let ext_ok = (1..=10).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    let suffix = if ext_ok {
        format!(".{ext}")
    } else {
        String::new()
    };

    let stem = clean_stem(suggested, &suffix);
    let stem = if stem.is_empty() {
        clean_stem(fallback_stem, &suffix)
    } else {
        stem
    };
    let stem = if stem.is_empty() {
        "output".to_owned()
    } else {
        stem
    };
    format!("{stem}{suffix}")
}

fn clean_stem(raw: &str, suffix: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut name: String = last.chars().filter(|&c| allowed(c)).collect();

    // Drop the extension if it's already the right one; we add it back.
    if !suffix.is_empty()
        && name.len() > suffix.len()
        && name.is_char_boundary(name.len() - suffix.len())
        && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    {
        name.truncate(name.len() - suffix.len());
    }

    let mut stem = trim(&name).to_owned();

    let budget = MAX_NAME_BYTES.saturating_sub(suffix.len());
    if stem.len() > budget {
        let mut cut = budget;
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        stem.truncate(cut);
        stem = trim(&stem).to_owned();
    }

    let device = stem.split('.').next().unwrap_or("").trim_end();
    if WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(device))
    {
        stem.insert(0, '_');
    }
    stem
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == '.' || c.is_whitespace())
}

fn allowed(c: char) -> bool {
    if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
        return false;
    }
    // Bidirectional controls, zero-width characters and the BOM.
    !matches!(c,
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2069}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
    )
}

#[cfg(test)]
mod tests {
    use super::sanitize_file_name as s;

    #[test]
    fn table() {
        let cases: &[(&str, &str, &str)] = &[
            ("report-unlocked.pdf", "pdf", "report-unlocked.pdf"),
            ("Report.PDF", "pdf", "Report.pdf"),
            ("report", "pdf", "report.pdf"),
            ("../../etc/passwd", "pdf", "passwd.pdf"),
            ("C:\\Windows\\evil.pdf", "pdf", "evil.pdf"),
            ("invoice\u{202E}fdp.exe", "pdf", "invoicefdp.exe.pdf"),
            ("a<b>c:d\"e|f?g*h.pdf", "pdf", "abcdefgh.pdf"),
            ("tab\there\n.pdf", "pdf", "tabhere.pdf"),
            ("  ..hidden..  ", "pdf", "hidden.pdf"),
            ("CON", "pdf", "_CON.pdf"),
            ("con.tar", "pdf", "_con.tar.pdf"),
            ("Lpt9 .pdf", "pdf", "_Lpt9.pdf"),
            ("console", "pdf", "console.pdf"),
            ("", "pdf", "fallback.pdf"),
            ("...", "pdf", "fallback.pdf"),
            ("/", "pdf", "fallback.pdf"),
            ("日本語の文書.pdf", "pdf", "日本語の文書.pdf"),
            ("x.pdf", "PDF", "x.pdf"),
            ("x.pdf", "../x", "x.pdf"),
        ];
        for (input, ext, want) in cases {
            assert_eq!(s(input, ext, "fallback"), *want, "{input:?}");
        }
    }

    #[test]
    fn long_names_are_cut_on_char_boundaries() {
        let out = s(&"é".repeat(500), "pdf", "f");
        assert!(out.len() <= 200);
        assert!(out.ends_with(".pdf"));
    }

    #[test]
    fn empty_fallback_still_yields_a_name() {
        assert_eq!(s("", "pdf", ""), "output.pdf");
    }
}
