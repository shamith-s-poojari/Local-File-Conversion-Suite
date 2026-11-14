import { describe, expect, test } from "vitest";
import type { Finding, Format, Targets } from "./api";
import type { FileView } from "./machines/app";
import { isAction, MAX_SUGGESTIONS, suggest } from "./suggestions";

function f(format: Format, opts: { size?: number; findings?: Finding["kind"][]; macro?: boolean } = {}): FileView {
  return {
    displayName: `x.${format}`,
    file: {
      fileId: Math.random().toString(36).slice(2),
      displayName: `x.${format}`,
      sizeBytes: opts.size ?? 1000,
      format,
      confidence: "certain",
      evidence: [],
      extensionMismatch: false,
      candidates: [],
      encryptionHint: "none",
      macroEnabled: opts.macro ?? false,
      cloudPlaceholder: false,
      actionable: true,
    },
    inspect: {
      pages: 0,
      encryption: "none",
      signed: null,
      warnings: [],
      findings: (opts.findings ?? []).map((kind) => ({ kind })),
    },
  };
}

const ALL_OPS = [
  "file-disarm", "file-clean", "media-clean", "image-compress", "pdf-merge", "images-to-pdf",
  "data-combine", "video-join", "audio-join", "video-frames",
];

function targets(ops = ALL_OPS, to: Format[] = ["jpg", "png", "pdf"]): Targets {
  const conversions = to.map((t) => ({ to: t, tier: "R" as const, bestEffort: false, exactWithTextLayer: false }));
  return {
    operations: ops.map((adapter) => ({ adapter, title: adapter, tier: "E" as const, bestEffort: false })),
    groups: [{ format: "jpg", files: 1, conversions }],
    shared: conversions,
  };
}

const ids = (files: FileView[], t: Targets | null = targets(), dup = 0) => suggest(files, t, dup).map((s) => s.id);

describe("suggest", () => {
  test.each<[string, FileView[], string[]]>([
    ["macro-enabled document", [f("docx", { macro: true })], ["disarm"]],
    ["PDF with JavaScript", [f("pdf", { findings: ["activeContent"] })], ["disarm"]],
    ["photo with location", [f("jpg", { findings: ["location"] })], ["clean"]],
    ["video with location: media cleaner, and a sheet", [f("mp4", { findings: ["location"] })], ["clean", "video-sheet"]],
    ["HEIC photos", [f("heic"), f("heic")], ["heic-jpg", "images-pdf"]],
    ["one big photo", [f("jpg", { size: 6 << 20 })], ["compress"]],
    ["two PDFs", [f("pdf"), f("pdf")], ["merge"]],
    ["two images", [f("png"), f("jpg")], ["images-pdf"]],
    ["two tables", [f("csv"), f("xlsx")], ["combine"]],
    ["two clips", [f("mp4"), f("mov")], ["join-video"]],
    ["two tracks", [f("mp3"), f("wav")], ["join-audio"]],
    ["one video", [f("mp4")], ["video-sheet"]],
    ["one plain PDF", [f("pdf")], []],
    ["a PDF and an image", [f("pdf"), f("png")], []],
  ])("%s", (_, files, expected) => {
    expect(ids(files)).toEqual(expected);
  });

  test("safety comes first and at most three are offered", () => {
    const files = [
      f("jpg", { findings: ["activeContent", "location"], size: 6 << 20 }),
      f("jpg", { size: 6 << 20 }),
    ];
    const out = ids(files);
    expect(out.length).toBe(MAX_SUGGESTIONS);
    expect(out[0]).toBe("disarm");
    expect(out).toEqual(["disarm", "clean", "compress"]);
  });

  test("a tool that isn't available isn't suggested", () => {
    const t = targets(ALL_OPS.filter((o) => o !== "pdf-merge"));
    expect(ids([f("pdf"), f("pdf")], t)).toEqual([]);
    const off = targets();
    off.operations.find((o) => o.adapter === "file-disarm")!.unavailable = { missingEngines: ["lxml"] };
    expect(ids([f("docx", { macro: true })], off)).toEqual([]);
  });

  test("location in a mix the cleaners can't take together gives no clean chip", () => {
    expect(ids([f("jpg", { findings: ["location"] }), f("mp4")])).toEqual([]);
  });

  test("duplicates are reported, not acted on", () => {
    const out = suggest([f("pdf"), f("pdf")], targets(), 1);
    expect(out.map((s) => s.id)).toEqual(["merge", "duplicates"]);
    const [merge, dup] = out;
    expect(dup?.text).toBe("1 file is a copy of another");
    expect(dup && isAction(dup)).toBe(false);
    expect(merge && isAction(merge)).toBe(true);
  });

  test("the compress chip asks for under 1 MB", () => {
    const [s] = suggest([f("jpg", { size: 6 << 20 })], targets());
    expect(s).toMatchObject({ operation: "image-compress", params: { targetKb: 1000 } });
  });

  test("no targets yet, no suggestions", () => {
    expect(ids([f("pdf"), f("pdf")], null)).toEqual([]);
  });
});
