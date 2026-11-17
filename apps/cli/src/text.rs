//! Human-readable text for error codes, reasons and warnings: the same
//! words the app shows, so the two front ends don't drift apart (audit U18).
//!
//! The tables below are copied from `apps/desktop/src/text.ts`. The test at
//! the bottom reads that file and fails when a code is missing here or a
//! text differs, so the fix after changing `text.ts` is to copy the new
//! text into the table (the failure names the entry).

/// One message: what happened, why, what to do.
pub struct Entry {
    pub code: &'static str,
    pub what: &'static str,
    pub why: Option<&'static str>,
    pub todo: Option<&'static str>,
}

impl Entry {
    fn text(&self) -> String {
        let mut s = self.what.to_string();
        for part in [self.why, self.todo].into_iter().flatten() {
            s.push(' ');
            s.push_str(part);
        }
        s
    }
}

fn find(table: &'static [Entry], key: &str) -> Option<&'static Entry> {
    table.iter().find(|e| e.code == key)
}

fn pair(table: &'static [(&str, &str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The message for a failure: the text of its reason when the app has one
/// (as the app's error box does), else the text of its code.
pub fn message(code: &str, reason: Option<&str>) -> String {
    if let Some(e) = reason.and_then(|r| find(REASONS, r)) {
        return e.text();
    }
    match find(ERRORS, code) {
        Some(e) => e.text(),
        None => format!("Something unexpected went wrong ({code})."),
    }
}

/// A job warning's text.
pub fn warning(code: &str) -> &'static str {
    pair(WARNINGS, code)
        .or_else(|| pair(WARNINGS, "W_UNKNOWN"))
        .unwrap_or("The engine reported an unusual warning.")
}

/// Why a file was left out when it was added (`FileResult::error`).
pub fn sniff_error(reason: &str) -> Option<&'static str> {
    pair(SNIFF_ERRORS, reason)
}

/// Files of a dropped folder that didn't fit under the 200-file limit.
pub fn folder_truncated(folder: &str, count: usize) -> String {
    let s = if count == 1 { "" } else { "s" };
    format!("{count} more file{s} in {folder} weren't added: up to 200 files at once.")
}

// ---- copied from apps/desktop/src/text.ts (checked by the test below) ----

pub const ERRORS: &[Entry] = &[
    Entry { code: "E_INPUT_CORRUPT", what: "This file can't be read.", why: Some("It's damaged, or it isn't really the type of file its name says."), todo: Some("Try opening it in another program and saving a fresh copy.") },
    Entry { code: "E_PASSWORD_REQUIRED", what: "This file needs a password.", why: None, todo: Some("Enter the password to open it.") },
    Entry { code: "E_PASSWORD_WRONG", what: "That password isn't right.", why: None, todo: Some("Check it and try again. Passwords can't be recovered or guessed by this app.") },
    Entry { code: "E_LIMIT_EXCEEDED", what: "This is larger or more complex than the app can handle safely.", why: Some("Fixed limits protect your computer from files that would use too much memory or disk. They can't be changed."), todo: Some("Try a smaller file, or split it into parts first.") },
    Entry { code: "E_RESOURCE_MEMORY", what: "Your computer ran out of memory for this job.", why: None, todo: Some("Close other programs and try again, or process fewer files at once.") },
    Entry { code: "E_RESOURCE_DISK", what: "There isn't enough free disk space.", why: None, todo: Some("Free up some space and try again.") },
    Entry { code: "E_TIMEOUT", what: "This took too long and was stopped.", why: Some("The file may be unusually complex, or the engine stopped responding."), todo: Some("Try again. If it keeps happening, try a smaller part of the file.") },
    Entry { code: "E_CANCELLED", what: "Cancelled.", why: None, todo: None },
    Entry { code: "E_PROTOCOL", what: "Something went wrong inside the app.", why: Some("A processing engine misbehaved and was stopped."), todo: Some("Try again. If it keeps happening, please report it.") },
    Entry { code: "E_OUTPUT_INVALID", what: "The result didn't pass its checks, so it wasn't kept.", why: None, todo: Some("Try again. If it keeps happening, please report it.") },
    Entry { code: "E_SANDBOX_UNAVAILABLE", what: "Processing is turned off.", why: Some("The app couldn't confirm its safety sandbox works on this computer, so it won't open files."), todo: Some("See Settings for the self-test result.") },
    Entry { code: "E_INSTALL_CORRUPT", what: "Part of the app is missing or damaged.", why: None, todo: Some("Reinstall the app.") },
    Entry { code: "E_REDACTION_VERIFICATION_FAILED", what: "Redaction couldn't be proven complete, so no file was produced.", why: Some("Independent checks found content that might still be recoverable."), todo: Some("You can try again with “turn affected pages into images”. Text on those pages will be lost.") },
    Entry { code: "E_NO_TEXT", what: "There's no text in this PDF to convert.", why: Some("Its pages are pictures of text, probably scans, so there are no words to pull out."), todo: Some("Convert it to Word, which keeps the pictures, or to images instead.") },
    Entry { code: "E_INTERNAL", what: "Something unexpected went wrong.", why: None, todo: Some("Try again. If it keeps happening, please report it.") },
    Entry { code: "E_PARAMS_INVALID", what: "Some settings aren't valid.", why: None, todo: Some("Check the highlighted fields.") },
    Entry { code: "E_CONFIRM_OVERWRITE", what: "Confirm replacing the original file.", why: None, todo: None },
    Entry { code: "E_BUSY", what: "This job is busy. Try again in a moment.", why: None, todo: None },
    Entry { code: "E_WRONG_STATE", what: "That can't be done at this step.", why: Some("The job has moved on, or hasn't reached the step that action needs."), todo: Some("Go back and try again.") },
    Entry { code: "E_UNKNOWN_JOB", what: "This job no longer exists.", why: None, todo: None },
    Entry { code: "E_USAGE", what: "That command isn't complete or isn't right.", why: Some("An option or argument is missing, or isn't one lfcs knows."), todo: Some("Run lfcs --help to see how to use it.") },
    Entry { code: "E_NO_USABLE_FILES", what: "None of these files can be used.", why: Some("They're missing, of a type the app doesn't handle, or couldn't be read."), todo: Some("Check the files and try again.") },
    Entry { code: "E_NO_ROUTE", what: "These files can't be made into that format.", why: None, todo: Some("Run lfcs targets to see what each file can become.") },
    Entry { code: "E_PARTIAL", what: "Some files couldn't be done. The others are ready.", why: None, todo: Some("The notes say which files failed and why.") },
    Entry { code: "E_SAVE_FAILED", what: "The results couldn't be saved.", why: Some("The folder may be read-only, full, or gone."), todo: Some("Choose another folder and try again.") },
    Entry { code: "E_UNKNOWN_FILE", what: "This file was removed.", why: None, todo: None },
];

pub const REASONS: &[Entry] = &[
    Entry {
        code: "emptyPaste",
        what: "There's nothing to paste.",
        why: None,
        todo: None,
    },
    Entry {
        code: "notAnImage",
        what: "Only PNG and JPEG pictures can be pasted.",
        why: None,
        todo: None,
    },
    Entry {
        code: "notText",
        what: "The pasted text couldn't be read.",
        why: None,
        todo: None,
    },
    Entry {
        code: "pasteTooLarge",
        what: "What you pasted is over 50 MB.",
        why: None,
        todo: None,
    },
    Entry {
        code: "tooManyFiles",
        what: "That's more than 200 files.",
        why: None,
        todo: None,
    },
    Entry {
        code: "allSkipped",
        what: "Every file is already in that format.",
        why: None,
        todo: Some("Choose a different format, or turn off skipping files that are already in it."),
    },
    Entry {
        code: "notCopyable",
        what: "Only pictures and text results can be copied.",
        why: None,
        todo: None,
    },
    Entry {
        code: "tooLargeToCopy",
        what: "The result is too large to copy.",
        why: None,
        todo: None,
    },
    Entry {
        code: "clipboardUnavailable",
        what: "The clipboard isn't available.",
        why: None,
        todo: None,
    },
    Entry {
        code: "revealFailed",
        what: "The folder couldn't be shown.",
        why: None,
        todo: Some("Open the folder you chose in your file manager."),
    },
    Entry {
        code: "downloadFailed",
        what: "This file couldn't be downloaded.",
        why: Some("It's stored online only, and your cloud service didn't send it."),
        todo: Some("Check your connection and that the cloud app is running, then try again."),
    },
    Entry {
        code: "engineMissing",
        what: "A program this needs isn't installed.",
        why: None,
        todo: Some("Install it, then reopen the app."),
    },
    Entry {
        code: "selfTestPending",
        what: "Checking the sandbox…",
        why: None,
        todo: Some("The safety check is still running. Try again in a moment."),
    },
    Entry {
        code: "passwordsDiffer",
        what: "These files need different passwords.",
        why: None,
        todo: Some("Convert them separately, so each gets its own."),
    },
    Entry {
        code: "officePassword",
        what: "This file is password protected.",
        why: Some("Word, Excel and PowerPoint files can't be opened with a password here."),
        todo: Some(
            "Remove the password in the program that made the file, save it, then add it again.",
        ),
    },
];

pub const WARNINGS: &[(&str, &str)] = &[
    ("FORMS_FLATTENED", "Form fields were turned into fixed content, or removed."),
    ("XFA_UNSUPPORTED", "This file has an XFA form, which may not display correctly."),
    ("SIGNATURE_INVALIDATED", "A digital signature in this file is no longer valid after this change."),
    ("TAGS_DROPPED", "Accessibility tags weren't kept."),
    ("OUTLINE_DROPPED", "Some bookmarks weren't kept."),
    ("ACTIVE_CONTENT_FOUND", "This file contains JavaScript or attachments. They are never run."),
    ("NO_TEXT_LAYER", "Some pages have no text (they may be scans), so search can't find words on them."),
    ("REDACTED_TEXT_REMAINS_ELSEWHERE", "Text you covered also appears elsewhere in the document."),
    ("GLYPH_UNSUPPORTED", "Some characters can't be shown in the watermark font and appear as “?”."),
    ("W_LARGE_JOB", "This job is larger than the memory budget, so it runs on its own."),
    ("TRANSPARENCY_FLATTENED", "Transparent areas were filled with a solid colour, because the new format has no transparency."),
    ("ANIMATION_FLATTENED", "Only the first frame of an animation was kept."),
    ("IMAGE_DOWNSCALED", "Some images were made smaller to fit the size or resolution you chose."),
    ("COLORS_REDUCED", "Colours were reduced to 256 at most: a GIF can hold no more, and it makes a PNG smaller."),
    ("METADATA_DROPPED", "Some details stored in the file (tags or properties) weren't kept."),
    ("AUDIO_RESAMPLED", "The audio was resampled to a rate the new format supports."),
    ("AUDIO_DOWNMIXED", "Surround sound was mixed down to fewer channels."),
    ("STREAMS_DROPPED", "Extra tracks (cover art, subtitles or other audio) weren't kept."),
    ("LAYOUT_APPROXIMATED", "The layout was rebuilt and may differ from the original. Check the result."),
    ("FORMULAS_DROPPED", "Formulas were replaced by their calculated values."),
    ("SHEETS_SPLIT", "Each sheet became its own file."),
    ("ONLY_FIRST_SHEET", "Only the first sheet was converted."),
    ("IMAGES_DROPPED", "Pictures weren't kept, because the new format holds text only."),
    ("EMBEDDED_FONTS_MISSING", "Some fonts aren't installed on this computer, so similar ones were used."),
    ("NESTED_DATA_FLATTENED", "Values with their own structure (lists or objects) were written as JSON text in a single cell."),
    ("COMMENTS_DROPPED", "Comments in the file weren't kept: the new format has no place for them."),
    ("REENCODED", "The files didn't match, so they were re-encoded to join them. This takes longer and loses a little quality."),
    ("KEPT_ORIGINAL", "Some files were already as small as they could get, so they were kept unchanged."),
    ("TARGET_NOT_REACHED", "Some files couldn't get under the size you chose. They were made as small as possible."),
    ("COLUMNS_MISMATCHED", "The files had different columns. Missing values were left empty."),
    ("ACTIVE_CONTENT_REMOVED", "Scripts, automatic actions or embedded programs were removed. What the file shows is unchanged."),
    ("MACROS_REMOVED", "Macros were removed. The file opens without asking to run code."),
    ("EXTERNAL_LINKS_REMOVED", "Things the file loaded from outside (templates, pictures, linked objects) were removed."),
    ("DETAILS_REMOVED", "Hidden details such as location, camera, author and dates were removed."),
    ("TRACKED_CHANGES_KEPT", "The file still has tracked changes, which show earlier wording. Accept or reject them in your editor first."),
    ("ENCRYPTION_REMOVED", "The original was password protected. This result isn't: protect it again if you need to."),
    ("WARNINGS_TRUNCATED", "There were more notes than can be shown here."),
    ("PART_FAILED", "Some files couldn't be done. The others are ready."),
    ("W_UNKNOWN", "The engine reported an unusual warning."),
];

pub const SNIFF_ERRORS: &[(&str, &str)] = &[
    ("empty", "The file is empty."),
    ("directory", "That's a folder, not a file."),
    ("specialFile", "That isn't a regular file."),
    ("notFound", "The file couldn't be found."),
    (
        "permissionDenied",
        "You don't have permission to read this file.",
    ),
    ("locked", "Another program has this file open."),
    ("unreadable", "The file couldn't be read."),
    ("emptyFolder", "The folder has no files that can be used."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    type Fields = BTreeMap<String, String>;

    #[derive(Debug, PartialEq)]
    enum Tok {
        Id(String),
        Str(String),
        P(char),
    }

    fn tokens(src: &str) -> Vec<Tok> {
        let c: Vec<char> = src.chars().collect();
        let (mut i, mut out) = (0, Vec::new());
        while i < c.len() {
            let ch = c[i];
            if ch.is_whitespace() {
                i += 1;
            } else if ch == '/' && c.get(i + 1) == Some(&'/') {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            } else if ch == '/' && c.get(i + 1) == Some(&'*') {
                i += 2;
                while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            } else if ch == '"' {
                let mut s = String::new();
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' {
                        i += 1;
                        match c[i] {
                            'n' => s.push('\n'),
                            'u' => {
                                let hex: String = c[i + 1..i + 5].iter().collect();
                                s.push(
                                    char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap(),
                                );
                                i += 4;
                            }
                            other => s.push(other),
                        }
                    } else {
                        s.push(c[i]);
                    }
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(s));
            } else if ch.is_alphanumeric() || ch == '_' || ch == '$' {
                let start = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$') {
                    i += 1;
                }
                out.push(Tok::Id(c[start..i].iter().collect()));
            } else {
                out.push(Tok::P(ch));
                i += 1;
            }
        }
        out
    }

    /// `export const NAME ... = { key: "text" | { what: "...", ... }, ... };`
    /// as key to fields; a bare string is the field `what`.
    fn parse_block(src: &str, name: &str) -> BTreeMap<String, Fields> {
        let start = src
            .find(&format!("export const {name}"))
            .unwrap_or_else(|| panic!("text.ts has no {name}"));
        let body = &src[start..];
        let open = body.find("= {").expect("an object literal") + 2;
        let t = tokens(&body[open..]);
        let mut it = t.into_iter().peekable();
        assert_eq!(it.next(), Some(Tok::P('{')));
        let mut out = BTreeMap::new();
        loop {
            let key = match it.next() {
                Some(Tok::Id(k)) | Some(Tok::Str(k)) => k,
                Some(Tok::P('}')) => break,
                Some(Tok::P(',')) => continue,
                other => panic!("{name}: unexpected {other:?}"),
            };
            assert_eq!(it.next(), Some(Tok::P(':')), "{name}.{key}");
            let mut fields = Fields::new();
            match it.next() {
                Some(Tok::Str(s)) => {
                    fields.insert("what".into(), s);
                }
                Some(Tok::P('{')) => loop {
                    match it.next() {
                        Some(Tok::Id(f)) => {
                            assert_eq!(it.next(), Some(Tok::P(':')));
                            match it.next() {
                                Some(Tok::Str(s)) => {
                                    fields.insert(f, s);
                                }
                                Some(_) => {} // `report: true`
                                None => panic!("{name}.{key}: cut off"),
                            }
                        }
                        Some(Tok::P(',')) => {}
                        Some(Tok::P('}')) => break,
                        other => panic!("{name}.{key}: unexpected {other:?}"),
                    }
                },
                other => panic!("{name}.{key}: unexpected {other:?}"),
            }
            out.insert(key, fields);
        }
        out
    }

    fn text_ts() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/src/text.ts");
        std::fs::read_to_string(&path).expect("apps/desktop/src/text.ts")
    }

    fn check_entries(name: &str, table: &[Entry]) {
        let ts = parse_block(&text_ts(), name);
        let rust: Vec<&str> = table.iter().map(|e| e.code).collect();
        for key in ts.keys() {
            assert!(
                rust.contains(&key.as_str()),
                "{name}.{key} is in text.ts but not in apps/cli/src/text.rs"
            );
        }
        for e in table {
            let f = ts
                .get(e.code)
                .unwrap_or_else(|| panic!("{name}.{} is in text.rs but not in text.ts", e.code));
            assert_eq!(
                Some(e.what),
                f.get("what").map(String::as_str),
                "{name}.{}",
                e.code
            );
            assert_eq!(
                e.why,
                f.get("why").map(String::as_str),
                "{name}.{} why",
                e.code
            );
            assert_eq!(
                e.todo,
                f.get("todo").map(String::as_str),
                "{name}.{} todo",
                e.code
            );
        }
    }

    fn check_pairs(name: &str, table: &[(&str, &str)]) {
        let ts = parse_block(&text_ts(), name);
        for key in ts.keys() {
            assert!(
                table.iter().any(|(k, _)| k == key),
                "{name}.{key} is in text.ts but not in apps/cli/src/text.rs"
            );
        }
        for (k, v) in table {
            let f = ts
                .get(*k)
                .unwrap_or_else(|| panic!("{name}.{k} is in text.rs but not in text.ts"));
            assert_eq!(Some(*v), f.get("what").map(String::as_str), "{name}.{k}");
        }
    }

    #[test]
    fn error_codes_match_text_ts() {
        check_entries("ERRORS", ERRORS);
    }

    #[test]
    fn reasons_match_text_ts() {
        check_entries("REASONS", REASONS);
    }

    #[test]
    fn warnings_match_text_ts() {
        check_pairs("WARNINGS", WARNINGS);
    }

    #[test]
    fn file_errors_match_text_ts() {
        check_pairs("SNIFF_ERRORS", SNIFF_ERRORS);
    }

    #[test]
    fn the_parser_reads_what_it_should() {
        let src = r#"export const X: Record<string, Message> = {
          A: { what: "It's \"quoted\".", todo: "Go “here”", report: true },
          // a comment
          b: "plain",
        };"#;
        let p = parse_block(src, "X");
        assert_eq!(p["A"]["what"], "It's \"quoted\".");
        assert_eq!(p["A"]["todo"], "Go \u{201c}here\u{201d}");
        assert_eq!(p["b"]["what"], "plain");
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn messages_prefer_the_reason() {
        assert!(message("E_INTERNAL", Some("tooManyFiles")).starts_with("That's more than 200"));
        assert!(message("E_INPUT_CORRUPT", None).starts_with("This file can't be read."));
        assert!(message("E_INPUT_CORRUPT", Some("noSuchReason")).starts_with("This file can't"));
        // Built at run time so the front end's scan for codes in use (text.test.ts)
        // doesn't take this made-up one for a real code.
        let unknown = format!("E_{}", "NOT_A_CODE");
        assert!(message(&unknown, None).contains(&unknown));
        assert!(warning("NOT_A_WARNING").contains("unusual"));
    }
}
