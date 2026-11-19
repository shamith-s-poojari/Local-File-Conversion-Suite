//! Sanitizing worker-suggested file names (spec section 7.4).

/// Longest name we produce, in bytes. Every OS we target allows 255; leave
/// room for a collision suffix such as ` (12)`.
const MAX_NAME_BYTES: usize = 200;

/// Whether Windows treats `name` as a device, whatever follows its first dot
/// (`CON.txt` is still the console) and however it is cased or padded.
///
/// Besides the classic names this covers `CONIN$`, `CONOUT$` and the
/// superscript forms `COM¹`/`LPT²`: Windows maps those to the same ports, so
/// a check on ASCII digits alone misses them. Used for names we write and for
/// names a worker claims, so it is the one definition.
pub(crate) fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split(['.', ':']).next().unwrap_or("");
    let stem = stem.trim_end_matches([' ', '.']).to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let Some(rest) = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
    else {
        return false;
    };
    let mut chars = rest.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('1'..='9' | '\u{B9}' | '\u{B2}' | '\u{B3}' | '\u{2074}'..='\u{2079}'), None)
    )
}

/// Turns a worker's `suggestedName` into a safe file name ending in
/// `.{extension}`.
///
/// - Keeps only the last path component, so `../../x` can't escape.
/// - Drops control characters, characters Windows forbids, and every
///   `Default_Ignorable` character (zero-width, bidi controls, variation
///   selectors, the soft hyphen) that could disguise the real extension.
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

    if is_windows_device_name(&stem) {
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
    // The interlinear annotation marks are not Default_Ignorable but are
    // invisible in the same way.
    !is_default_ignorable(c) && !matches!(c, '\u{FFF9}'..='\u{FFFB}')
}

/// The Unicode `Default_Ignorable_Code_Point` property (Unicode 15), written
/// out so no crate is needed: zero-width and bidi controls, variation
/// selectors, the soft hyphen, fillers and tag characters. They render as
/// nothing, so they can hide an extension or make two names look the same.
fn is_default_ignorable(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{034F}'
        | '\u{061C}'
        | '\u{115F}'..='\u{1160}'
        | '\u{17B4}'..='\u{17B5}'
        | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}'
        | '\u{3164}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FEFF}'
        | '\u{FFA0}'
        | '\u{FFF0}'..='\u{FFF8}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0000}'..='\u{E0FFF}'
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
            ("COM\u{B9}", "pdf", "_COM\u{B9}.pdf"),
            ("lpt\u{B3}.txt", "pdf", "_lpt\u{B3}.txt.pdf"),
            ("CONIN$", "pdf", "_CONIN$.pdf"),
            ("conout$.log", "pdf", "_conout$.log.pdf"),
            ("COM0", "pdf", "COM0.pdf"),
            ("COM10", "pdf", "COM10.pdf"),
            // Invisible characters can't hide a device name or an extension.
            ("C\u{200B}ON", "pdf", "_CON.pdf"),
            ("a\u{AD}b\u{FE0F}c\u{E0041}d\u{3164}.pdf", "pdf", "abcd.pdf"),
            ("x\u{2065}\u{FFF0}\u{1D173}.pdf", "pdf", "x.pdf"),
            ("report\u{180E}.exe", "pdf", "report.exe.pdf"),
        ];
        for (input, ext, want) in cases {
            assert_eq!(s(input, ext, "fallback"), *want, "{input:?}");
        }
    }

    #[test]
    fn device_names_ignore_case_padding_and_extensions() {
        use super::is_windows_device_name as dev;
        for yes in [
            "con",
            "NuL.txt",
            "aux .x",
            "COM9.",
            "Lpt\u{B2}",
            "com\u{2079}",
            "CONOUT$",
        ] {
            assert!(dev(yes), "{yes:?}");
        }
        for no in ["console", "com", "COM12", "lpt", "con-1", "x.con", ""] {
            assert!(!dev(no), "{no:?}");
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
