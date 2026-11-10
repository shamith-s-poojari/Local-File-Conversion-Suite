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
};

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
