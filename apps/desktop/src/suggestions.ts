// Smart suggestions (ADR 0005): up to three one-click picks drawn from what
// the selection is and what inspect found. Pure, so the rules are tested
// on their own; the Configure screen turns a click into the usual
// SELECT_OPERATION / SELECT_CONVERSION.

import type { ConversionTarget, Format, OperationTarget, Targets } from "./api";
import { familyOf } from "./formats";
import type { FileView } from "./machines/app";

export type Suggestion =
  | { id: string; text: string; operation: string; params: Record<string, unknown> }
  | { id: string; text: string; to: Format };

export const MAX_SUGGESTIONS = 3;
const LARGE_IMAGE = 5 * 1024 * 1024;

const CLEANABLE_IMAGES: Format[] = ["jpg", "png", "webp", "tiff", "avif"];
const CLEANABLE_DOCS: Format[] = ["pdf", "docx", "xlsx", "pptx"];
const MEDIA_CLEANABLE: Format[] = [
  "mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "aiff", "mp4", "mov", "mkv", "webm",
];
const SHEETS: Format[] = ["csv", "xlsx"];

function operation(t: Targets, adapter: string): OperationTarget | undefined {
  return t.operations.find((o) => o.adapter === adapter && !o.unavailable);
}

function conversion(t: Targets, to: Format): ConversionTarget | undefined {
  const all = t.groups.length > 1 ? t.shared : [...t.shared, ...t.groups.flatMap((g) => g.conversions)];
  return all.find((c) => c.to === to && !c.unavailable && !c.bestEffort);
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * `duplicates` is how many selected files repeat an earlier one (from the
 * core's `duplicates` command); 0 or undefined when none do.
 */
export function suggest(files: FileView[], t: Targets | null, duplicates = 0): Suggestion[] {
  if (!t) return [];
  const ready = files.filter((f) => f.file?.actionable);
  if (ready.length === 0) return [];
  const formats = ready.map((f) => f.file!.format);
  const kinds = (f: FileView) => new Set(f.inspect?.findings.map((x) => x.kind) ?? []);
  const every = (pred: (f: Format) => boolean) => formats.every(pred);
  const out: Suggestion[] = [];
  const add = (s: Suggestion | null | undefined) => {
    if (s && out.length < MAX_SUGGESTIONS && !out.some((o) => o.id === s.id)) out.push(s);
  };

  // Safety first: something in the file runs, or loads from outside, when opened.
  const risky = ready.filter((f) => {
    const k = kinds(f);
    return f.file!.macroEnabled || k.has("activeContent") || k.has("macros");
  });
  if (risky.length > 0 && operation(t, "file-disarm")) {
    add({
      id: "disarm",
      text: risky.length === ready.length ? "Make safe to open" : `Make safe to open (${plural(risky.length, "file")} can run code)`,
      operation: "file-disarm",
      params: {},
    });
  }

  // Location before sharing.
  const located = ready.filter((f) => kinds(f).has("location"));
  if (located.length > 0) {
    const media = every((f) => MEDIA_CLEANABLE.includes(f));
    const still = every((f) => CLEANABLE_IMAGES.includes(f) || CLEANABLE_DOCS.includes(f));
    const adapter = media ? "media-clean" : still ? "file-clean" : null;
    if (adapter && operation(t, adapter)) {
      add({ id: "clean", text: "Remove location before sharing", operation: adapter, params: {} });
    }
  }

  // HEIC opens on Apple devices; JPG everywhere.
  if (every((f) => f === "heic") && conversion(t, "jpg")) {
    add({ id: "heic-jpg", text: "Convert to JPG so every device can open them", to: "jpg" });
  }

  // Big photos.
  const images = ready.filter((f) => familyOf(f.file!.format) === "image");
  if (
    images.length === ready.length &&
    images.some((f) => f.file!.sizeBytes > LARGE_IMAGE) &&
    operation(t, "image-compress")
  ) {
    add({ id: "compress", text: "Make them smaller than 1 MB", operation: "image-compress", params: { targetKb: 1000 } });
  }

  // Several of a kind: put them together.
  if (ready.length >= 2) {
    if (every((f) => f === "pdf") && operation(t, "pdf-merge")) {
      add({ id: "merge", text: `Merge ${plural(ready.length, "PDF")} into one`, operation: "pdf-merge", params: {} });
    } else if (images.length === ready.length && operation(t, "images-to-pdf")) {
      add({ id: "images-pdf", text: `Put ${plural(ready.length, "image")} in one PDF`, operation: "images-to-pdf", params: {} });
    } else if (every((f) => SHEETS.includes(f)) && operation(t, "data-combine")) {
      add({ id: "combine", text: `Combine ${plural(ready.length, "table")} into one`, operation: "data-combine", params: {} });
    } else if (every((f) => familyOf(f) === "video") && operation(t, "video-join")) {
      add({ id: "join-video", text: `Join ${plural(ready.length, "clip")} into one video`, operation: "video-join", params: {} });
    } else if (every((f) => familyOf(f) === "audio") && operation(t, "audio-join")) {
      add({ id: "join-audio", text: `Join ${plural(ready.length, "track")} into one`, operation: "audio-join", params: {} });
    }
  }

  // One video: a sheet of stills shows what's in it at a glance.
  if (ready.length === 1 && formats[0] && familyOf(formats[0]) === "video" && operation(t, "video-frames")) {
    add({
      id: "video-sheet",
      text: "Contact sheet of the video",
      operation: "video-frames",
      params: { layout: "sheet", mode: "count", count: 12, to: "jpg" },
    });
  }

  if (duplicates > 0) {
    add({ id: "duplicates", text: `${plural(duplicates, "file")} ${duplicates === 1 ? "is a copy" : "are copies"} of another`, operation: "", params: {} });
  }
  return out;
}

/** The duplicates note is information, not an action. */
export function isAction(s: Suggestion): boolean {
  return "to" in s || s.operation !== "";
}
