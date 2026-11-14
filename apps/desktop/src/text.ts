// User text for every code (spec 13.1): what happened, why, what to do.
// One file, so a check can find codes that have no text.

export interface Message {
  what: string;
  why?: string;
  todo?: string;
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
    what: "This is larger than the app allows.",
    why: "Limits protect your computer from files that would use too much memory or disk.",
    todo: "Raise the limit in Settings, or split the file first.",
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
  },
  E_OUTPUT_INVALID: {
    what: "The result didn't pass its checks, so it wasn't kept.",
    todo: "Try again. If it keeps happening, please report it.",
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
  },
  E_PARAMS_INVALID: {
    what: "Some settings aren't valid.",
    todo: "Check the highlighted fields.",
  },
  E_CONFIRM_OVERWRITE: { what: "Confirm replacing the original file." },
  E_BUSY: { what: "This job is busy. Try again in a moment." },
  E_UNKNOWN_JOB: { what: "This job no longer exists." },
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

/** Reasons the paste, copy and skip commands give (CommandError.reason). */
export const ACTION_ERRORS: Record<string, string> = {
  emptyPaste: "There's nothing to paste.",
  notAnImage: "Only PNG and JPEG pictures can be pasted.",
  notText: "The pasted text couldn't be read.",
  pasteTooLarge: "What you pasted is over 50 MB.",
  tooManyFiles: "That's more than 200 files.",
  allSkipped: "Every file is already in that format.",
  notCopyable: "Only pictures and text results can be copied.",
  tooLargeToCopy: "The result is too large to copy.",
  clipboardUnavailable: "The clipboard isn't available.",
};

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

export function warningText(code: string): string {
  return WARNINGS[code] ?? WARNINGS.W_UNKNOWN!;
}

export const TIER_LABELS: Record<string, string> = {
  E: "Exact",
  H: "High fidelity",
  R: "Best effort",
  L: "Lossy",
};

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** How to install an engine the app looks for (ADR 0003), in this OS's
 * usual way. The app finds it on its next start. */
export function installHint(engine: "ffmpeg" | "libreoffice", platform: string | undefined): string {
  const name = engine === "ffmpeg" ? "FFmpeg" : "LibreOffice";
  const command: Record<string, Record<string, string>> = {
    ffmpeg: {
      macos: "brew install ffmpeg",
      windows: "winget install Gyan.FFmpeg",
      linux: "sudo apt install ffmpeg",
    },
    libreoffice: {
      macos: "brew install --cask libreoffice",
      windows: "winget install TheDocumentFoundation.LibreOffice",
      linux: "sudo apt install libreoffice",
    },
  };
  const cmd = platform ? command[engine]?.[platform] : undefined;
  return cmd ? `Needs ${name}: install it with \u201c${cmd}\u201d, then reopen the app.` : `Needs ${name}.`;
}
