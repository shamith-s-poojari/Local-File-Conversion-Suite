// User text for every code (spec 13.1): what happened, why, what to do.
// One file, so a check can find codes that have no text.

export interface Message {
  what: string;
  why?: string;
  todo?: string;
  /** The text asks the person to report the problem: offer "Copy
   * diagnostics" beside it (audit U30). */
  report?: boolean;
}

export const ERRORS: Record<string, Message> = {
  E_INPUT_CORRUPT: {
    what: "This file can't be read.",
    why: "It's damaged, or it isn't really the type of file its name says.",
    todo: "Try opening it in another program and saving a fresh copy.",
  },
  E_PASSWORD_REQUIRED: {
    what: "This file needs a password.",
    todo: "Enter the password to open it.",
  },
  E_PASSWORD_WRONG: {
    what: "That password isn't right.",
    todo: "Check it and try again. Passwords can't be recovered or guessed by this app.",
  },
  E_LIMIT_EXCEEDED: {
    what: "This is larger or more complex than the app can handle safely.",
    why: "Fixed limits protect your computer from files that would use too much memory or disk. They can't be changed.",
    todo: "Try a smaller file, or split it into parts first.",
  },
  E_RESOURCE_MEMORY: {
    what: "Your computer ran out of memory for this job.",
    todo: "Close other programs and try again, or process fewer files at once.",
  },
  E_RESOURCE_DISK: {
    what: "There isn't enough free disk space.",
    todo: "Free up some space and try again.",
  },
  E_TIMEOUT: {
    what: "This took too long and was stopped.",
    why: "The file may be unusually complex, or the engine stopped responding.",
    todo: "Try again. If it keeps happening, try a smaller part of the file.",
  },
  E_CANCELLED: { what: "Cancelled." },
  E_PROTOCOL: {
    what: "Something went wrong inside the app.",
    why: "A processing engine misbehaved and was stopped.",
    todo: "Try again. If it keeps happening, please report it.",
    report: true,
  },
  E_OUTPUT_INVALID: {
    what: "The result didn't pass its checks, so it wasn't kept.",
    todo: "Try again. If it keeps happening, please report it.",
    report: true,
  },
  E_SANDBOX_UNAVAILABLE: {
    what: "Processing is turned off.",
    why: "The app couldn't confirm its safety sandbox works on this computer, so it won't open files.",
    todo: "See Settings for the self-test result.",
  },
  E_INSTALL_CORRUPT: {
    what: "Part of the app is missing or damaged.",
    todo: "Reinstall the app.",
  },
  E_REDACTION_VERIFICATION_FAILED: {
    what: "Redaction couldn't be proven complete, so no file was produced.",
    why: "Independent checks found content that might still be recoverable.",
    todo: "You can try again with “turn affected pages into images”. Text on those pages will be lost.",
  },
  E_NO_TEXT: {
    what: "There's no text in this PDF to convert.",
    why: "Its pages are pictures of text, probably scans, so there are no words to pull out.",
    todo: "Convert it to Word, which keeps the pictures, or to images instead.",
  },
  E_INTERNAL: {
    what: "Something unexpected went wrong.",
    todo: "Try again. If it keeps happening, please report it.",
    report: true,
  },
  E_PARAMS_INVALID: {
    what: "Some settings aren't valid.",
    todo: "Check the highlighted fields.",
  },
  E_CONFIRM_OVERWRITE: { what: "Confirm replacing the original file." },
  E_BUSY: { what: "This job is busy. Try again in a moment." },
  E_WRONG_STATE: {
    what: "That can't be done at this step.",
    why: "The job has moved on, or hasn't reached the step that action needs.",
    todo: "Go back and try again.",
  },
  E_UNKNOWN_JOB: { what: "This job no longer exists." },
  E_USAGE: {
    what: "That command isn't complete or isn't right.",
    why: "An option or argument is missing, or isn't one lfcs knows.",
    todo: "Run lfcs --help to see how to use it.",
  },
  E_NO_USABLE_FILES: {
    what: "None of these files can be used.",
    why: "They're missing, of a type the app doesn't handle, or couldn't be read.",
    todo: "Check the files and try again.",
  },
  E_NO_ROUTE: {
    what: "These files can't be made into that format.",
    todo: "Run lfcs targets to see what each file can become.",
  },
  E_PARTIAL: {
    what: "Some files couldn't be done. The others are ready.",
    todo: "The notes say which files failed and why.",
  },
  E_SAVE_FAILED: {
    what: "The results couldn't be saved.",
    why: "The folder may be read-only, full, or gone.",
    todo: "Choose another folder and try again.",
  },
  E_UNKNOWN_FILE: { what: "This file was removed." },
};

export const WARNINGS: Record<string, string> = {
  FORMS_FLATTENED: "Form fields were turned into fixed content, or removed.",
  XFA_UNSUPPORTED: "This file has an XFA form, which may not display correctly.",
  SIGNATURE_INVALIDATED: "A digital signature in this file is no longer valid after this change.",
  TAGS_DROPPED: "Accessibility tags weren't kept.",
  OUTLINE_DROPPED: "Some bookmarks weren't kept.",
  ACTIVE_CONTENT_FOUND: "This file contains JavaScript or attachments. They are never run.",
  NO_TEXT_LAYER: "Some pages have no text (they may be scans), so search can't find words on them.",
  REDACTED_TEXT_REMAINS_ELSEWHERE: "Text you covered also appears elsewhere in the document.",
  GLYPH_UNSUPPORTED: "Some characters can't be shown in the watermark font and appear as “?”.",
  W_LARGE_JOB: "This job is larger than the memory budget, so it runs on its own.",
  TRANSPARENCY_FLATTENED: "Transparent areas were filled with a solid colour, because the new format has no transparency.",
  ANIMATION_FLATTENED: "Only the first frame of an animation was kept.",
  IMAGE_DOWNSCALED: "Some images were made smaller to fit the size or resolution you chose.",
  COLORS_REDUCED: "Colours were reduced to 256 at most: a GIF can hold no more, and it makes a PNG smaller.",
  METADATA_DROPPED: "Some details stored in the file (tags or properties) weren't kept.",
  AUDIO_RESAMPLED: "The audio was resampled to a rate the new format supports.",
  AUDIO_DOWNMIXED: "Surround sound was mixed down to fewer channels.",
  STREAMS_DROPPED: "Extra tracks (cover art, subtitles or other audio) weren't kept.",
  LAYOUT_APPROXIMATED: "The layout was rebuilt and may differ from the original. Check the result.",
  FORMULAS_DROPPED: "Formulas were replaced by their calculated values.",
  SHEETS_SPLIT: "Each sheet became its own file.",
  ONLY_FIRST_SHEET: "Only the first sheet was converted.",
  IMAGES_DROPPED: "Pictures weren't kept, because the new format holds text only.",
  EMBEDDED_FONTS_MISSING: "Some fonts aren't installed on this computer, so similar ones were used.",
  NESTED_DATA_FLATTENED: "Values with their own structure (lists or objects) were written as JSON text in a single cell.",
  COMMENTS_DROPPED: "Comments in the file weren't kept: the new format has no place for them.",
  REENCODED: "The files didn't match, so they were re-encoded to join them. This takes longer and loses a little quality.",
  KEPT_ORIGINAL: "Some files were already as small as they could get, so they were kept unchanged.",
  TARGET_NOT_REACHED: "Some files couldn't get under the size you chose. They were made as small as possible.",
  COLUMNS_MISMATCHED: "The files had different columns. Missing values were left empty.",
  ACTIVE_CONTENT_REMOVED: "Scripts, automatic actions or embedded programs were removed. What the file shows is unchanged.",
  MACROS_REMOVED: "Macros were removed. The file opens without asking to run code.",
  EXTERNAL_LINKS_REMOVED: "Things the file loaded from outside (templates, pictures, linked objects) were removed.",
  DETAILS_REMOVED: "Hidden details such as location, camera, author and dates were removed.",
  TRACKED_CHANGES_KEPT: "The file still has tracked changes, which show earlier wording. Accept or reject them in your editor first.",
  ENCRYPTION_REMOVED: "The original was password protected. This result isn't: protect it again if you need to.",
  WARNINGS_TRUNCATED: "There were more notes than can be shown here.",
  PART_FAILED: "Some files couldn't be done. The others are ready.",
  W_UNKNOWN: "The engine reported an unusual warning.",
};

export const SNIFF_ERRORS: Record<string, string> = {
  empty: "The file is empty.",
  directory: "That's a folder, not a file.",
  specialFile: "That isn't a regular file.",
  notFound: "The file couldn't be found.",
  permissionDenied: "You don't have permission to read this file.",
  locked: "Another program has this file open.",
  unreadable: "The file couldn't be read.",
  emptyFolder: "The folder has no files that can be used.",
};

/** A folder had more files than fit under the 200-file limit. */
export function folderTruncatedText(count: number | undefined): string {
  const n = count ?? 0;
  return `${n} more file${n === 1 ? "" : "s"} in this folder weren't added: up to 200 files at once.`;
}

/** Reasons a command gives (CommandError.reason) that a person can act on or
 * should read about. A reason with no text here is in REASONS_USING_CODE_TEXT:
 * the text of its code says enough (audit U5). */
export const REASONS: Record<string, Message> = {
  emptyPaste: { what: "There's nothing to paste." },
  notAnImage: { what: "Only PNG and JPEG pictures can be pasted." },
  notText: { what: "The pasted text couldn't be read." },
  pasteTooLarge: { what: "What you pasted is over 50 MB." },
  tooManyFiles: { what: "That's more than 200 files." },
  allSkipped: {
    what: "Every file is already in that format.",
    todo: "Choose a different format, or turn off skipping files that are already in it.",
  },
  notCopyable: { what: "Only pictures and text results can be copied." },
  tooLargeToCopy: { what: "The result is too large to copy." },
  clipboardUnavailable: { what: "The clipboard isn't available." },
  revealFailed: {
    what: "The folder couldn't be shown.",
    todo: "Open the folder you chose in your file manager.",
  },
  downloadFailed: {
    what: "This file couldn't be downloaded.",
    why: "It's stored online only, and your cloud service didn't send it.",
    todo: "Check your connection and that the cloud app is running, then try again.",
  },
  engineMissing: {
    what: "A program this needs isn't installed.",
    todo: "Install it, then reopen the app.",
  },
  selfTestPending: {
    what: "Checking the sandbox…",
    todo: "The safety check is still running. Try again in a moment.",
  },
  passwordsDiffer: {
    what: "These files need different passwords.",
    todo: "Convert them separately, so each gets its own.",
  },
  officePassword: {
    what: "This file is password protected.",
    why: "Word, Excel and PowerPoint files can't be opened with a password here.",
    todo: "Remove the password in the program that made the file, save it, then add it again.",
  },
};

/** Reasons with no text of their own: they come with a code whose text fits
 * (an internal mistake, a damaged install, a full disk), or the person
 * can't do anything about them. The text test makes every reason the core
 * can give be in REASONS or here. */
export const REASONS_USING_CODE_TEXT = [
  "appNotStarted", "badJobId", "badParamsSchema", "commandPanicked", "jobNotFinished", "multiHop",
  "noFiles", "noOutputs", "noRoute", "nothingPending", "notACandidate", "notActionable",
  "notAnObject", "notAnOperation", "notPreviewable", "orderMismatch", "passwordNotText",
  "pasteFolder", "pasteName", "pasteWrite", "previewFolder", "selfTestFailed", "unknownAdapter",
  "unknownFile", "unknownFormat", "unknownJob", "unknownOperation", "unknownOutput",
  "unknownOutputFormat", "unknownPasteKind", "unreadable", "unreadableOutput", "unsupportedDpi",
  "unsupportedInput", "worker", "workerStart", "wrongFileCount",
];

/** What each kind of finding is called in "Reveals: …" (ADR 0005). */
export const FINDING_LABELS: Record<string, string> = {
  location: "location",
  device: "camera or device",
  author: "author",
  software: "software",
  dates: "dates",
  comments: "comments",
  trackedChanges: "tracked changes",
  hiddenContent: "hidden content",
  tags: "tags",
  activeContent: "active content",
  macros: "macros",
  externalLinks: "outside links",
  embeddedFiles: "embedded files",
};

/** Kinds that mean the file can do something when it's opened. */
export const ACTIVE_KINDS = ["activeContent", "macros", "externalLinks", "embeddedFiles"];

export function errorMessage(code: string): Message {
  return ERRORS[code] ?? ERRORS.E_INTERNAL!;
}

/** The text for an error: its reason's when the app has one, else its code's. */
export function reasonMessage(code: string, reason?: string | null): Message {
  return (reason ? REASONS[reason] : undefined) ?? errorMessage(code);
}

export function warningText(code: string): string {
  return WARNINGS[code] ?? WARNINGS.W_UNKNOWN!;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** How to install an engine the app looks for (ADR 0003), in this OS's
 * usual way. The app finds it on its next start. */
export function installHint(engine: "ffmpeg" | "libreoffice", platform: string | undefined): string {
  const name = ENGINE_NAMES[engine];
  const cmd = installCommand(engine, platform);
  return cmd ? `Needs ${name}: install it with \u201c${cmd}\u201d, then reopen the app.` : `Needs ${name}.`;
}

export const ENGINE_NAMES = { ffmpeg: "FFmpeg", libreoffice: "LibreOffice" } as const;

/** The command alone, for showing as text a person can copy. */
export function installCommand(engine: "ffmpeg" | "libreoffice", platform: string | undefined): string | undefined {
  const command: Record<string, Record<string, string>> = {
    ffmpeg: {
      macos: "brew install ffmpeg-full",
      windows: "winget install Gyan.FFmpeg",
      linux: "sudo apt install ffmpeg",
    },
    libreoffice: {
      macos: "brew install --cask libreoffice",
      windows: "winget install TheDocumentFoundation.LibreOffice",
      linux: "sudo apt install libreoffice",
    },
  };
  return platform ? command[engine]?.[platform] : undefined;
}

/** Shown when files are dropped while a job is in progress (audit U2). */
export const FINISH_FIRST = "Finish this first. The files you dropped weren't added: drop them again afterwards.";

/** A conversion left some files out because they were already in the target format. */
export function skippedText(count: number, formatLabel: string): string {
  return `Skipped ${count} already in ${formatLabel}.`;
}

/** What the redaction checks (V1 to V8, `verified-V1` …) look for, in plain
 * words. A check this list doesn't know is shown under its ID. */
export const CHECK_NAMES: Record<string, string> = {
  V1: "No text left under the covered areas",
  V2: "Covered areas checked a second way",
  V3: "Covered words gone from the page text",
  V4: "Covered words gone from hidden file data",
  V5: "No hidden extras (properties, attachments, scripts)",
  V6: "Covered areas are solid",
  V7: "No earlier version kept inside the file",
  V8: "Page count unchanged",
};

export function checkName(id: string): string {
  return CHECK_NAMES[id] ?? `Check ${id}`;
}

/** The sandbox self-test's checks, by the name Rust gives them. */
export const SELFTEST_NAMES: Record<string, string> = {
  runtime_readable: "Can read its own runtime",
  read_sentinel: "Can't read files outside the job",
  list_home: "Can't list your home folder",
  read_other_job: "Can't read another job's files",
  write_outside: "Can't write outside its own folder",
  connect_external: "Has no internet access",
  connect_loopback: "Can't reach services on this computer",
  dns: "Can't look up web addresses",
  spawn_shell: "Can't start a shell",
  read_job: "Can read the job's files",
  write_work: "Can write in its work folder",
  nothing_written_outside: "Nothing was written outside the job",
  memory_limit: "Is stopped when it uses too much memory",
};

export function selfTestName(name: string): string {
  return SELFTEST_NAMES[name] ?? name.replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase());
}

/** What the "Needs: …" line under a disabled Convert button says. */
export const NEEDS = {
  prefix: "Needs:",
  file: "a file",
  choice: "a format or a tool",
  loading: "the tool list to load",
  confirmPassword: "confirm password",
  passwordsMatch: "matching passwords",
};

/** Messages under a field the core (or the form) found invalid (audit U27). */
export const FIELD_ERRORS = {
  generic: "Check this setting.",
  choice: "Choose one of the options.",
  color: "Enter a colour like #1F6C9F.",
  password: "Enter a password.",
  mismatch: "The passwords don't match.",
  number: "Enter a number.",
  text: "Check this text.",
  numberBetween: (min: number, max: number) => `Enter a number from ${min} to ${max}.`,
  numberAtLeast: (min: number) => `Enter a number of ${min} or more.`,
  numberAtMost: (max: number) => `Enter a number of ${max} or less.`,
  textBetween: (min: number, max: number) => `Use between ${min} and ${max} characters.`,
  textAtLeast: (min: number) => `Use at least ${min} characters.`,
  textAtMost: (max: number) => `Use at most ${max} characters.`,
};

export const CONFIRM_LABEL = "Confirm password";

/** Shown when the list of what the files can become couldn't be loaded. */
export const TARGETS_FAILED = "Couldn't work out what these files can become.";

/** The redaction canvas: drawing needs a pointer, but nothing else does. */
export const REDACTION_KEYBOARD_HINT =
  "From the keyboard: use “Redact this whole page”, or add search terms below, which are removed from the whole document.";

export const PAGE_ORGANIZER = {
  moved: (page: number, position: number, total: number) => `Page ${page} moved to position ${position} of ${total}.`,
  rotated: (page: number, degrees: number) => `Page ${page} rotated to ${degrees}°.`,
  duplicated: (page: number, total: number) => `Page ${page} duplicated. ${total} pages now.`,
  deleted: (page: number, total: number) => `Page ${page} deleted. ${total} pages left.`,
  reset: (total: number) => `Back to the original order. ${total} pages.`,
};

export const DIAGNOSTICS = {
  copy: "Copy diagnostics",
  copied: "Copied",
  failed: "Couldn't copy. Select the text below and copy it yourself.",
  note: "Version, system and test results only: no file names or contents.",
};

/** The Saved screen (audit U7). */
export const SAVED = {
  where: "Saved to",
  show: "Show in folder",
};

/** Declining to replace an original when saving (audit S16). */
export const OVERWRITE_KEEP = "Keep the original";

/** A file in "Partly done" whose position the app can't match to a name. */
export function unnamedFile(index: number): string {
  return `File ${index + 1}`;
}

/** What a safe detail of a warning reads as. Only numbers and page lists
 * are shown: other details can hold document text (audit U8). */
export function warningDetailText(detail: unknown): string | null {
  if (typeof detail !== "object" || detail === null) return null;
  const d = detail as Record<string, unknown>;
  const parts: string[] = [];
  if (typeof d.count === "number" && Number.isFinite(d.count)) parts.push(`${d.count} found.`);
  if (Array.isArray(d.pages) && d.pages.length > 0 && d.pages.every((p) => Number.isInteger(p))) {
    const shown = (d.pages as number[]).slice(0, 20).join(", ");
    const more = d.pages.length > 20 ? ` and ${d.pages.length - 20} more` : "";
    parts.push(`Pages: ${shown}${more}.`);
  }
  if (typeof d.maxSide === "number" && Number.isFinite(d.maxSide)) parts.push(`Longest side: ${d.maxSide} px.`);
  return parts.length > 0 ? parts.join(" ") : null;
}

/** Home banner when the core couldn't start at all (for example, a build
 * with no worker runtime). It isn't a sandbox failure, so it doesn't use the
 * E_SANDBOX_UNAVAILABLE heading; the core's own reason, written for
 * developers, goes in a disclosure. */
export const STARTUP_FAILED = {
  what: "Part of the app is missing, so it can't open files.",
  todo: "Reinstall the app. If it keeps happening, please report it with the details below.",
  details: "Details",
};
