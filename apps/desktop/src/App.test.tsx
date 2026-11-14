import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import type { Api, FileResult, InspectInfo } from "./api";
import { App, inspectAll } from "./App";
import { SchemaForm } from "./features/SchemaForm";

test("home screen states that processing is local", () => {
  expect(renderToStaticMarkup(<App />)).toContain("Processed on your device");
});

const file = (id: string): FileResult => ({
  displayName: `${id}.pdf`,
  file: {
    fileId: id,
    displayName: `${id}.pdf`,
    sizeBytes: 1,
    format: "pdf",
    confidence: "certain",
    evidence: [],
    extensionMismatch: false,
    candidates: [],
    encryptionHint: "maybe",
    macroEnabled: false,
    cloudPlaceholder: false,
    actionable: true,
  },
});

test("a file that needs an open password asks for it", async () => {
  // Found by running the app: inspect reports such files as 0 pages, not an error.
  const api = {
    inspectFile: async (id: string): Promise<InspectInfo> =>
      id === "locked"
        ? { pages: 0, encryption: "openPassword", signed: null, warnings: [], findings: [] }
        : { pages: 3, encryption: "none", signed: false, warnings: [], findings: [] },
  } as unknown as Api;
  const [locked, open] = await inspectAll(api, [file("locked"), file("open")]);
  expect(locked?.needs).toBe("password");
  expect(locked?.inspect).toBeUndefined();
  expect(open?.needs).toBeUndefined();
  expect(open?.inspect?.pages).toBe(3);
});

test("an invalid page range blocks Start", async () => {
  const { missingRequired } = await import("./features/SchemaForm");
  const schema = { type: "object", properties: { pages: { type: "string", "x-ui": "pageRanges" } } };
  expect(missingRequired(schema, { pages: "5" }, 3)).toEqual(["pages"]);
  expect(missingRequired(schema, { pages: "1-2" }, 3)).toEqual([]);
  expect(missingRequired(schema, {}, 3)).toEqual([]);
});

test("fields with x-when show only for matching params", () => {
  const schema = {
    type: "object",
    properties: {
      mode: { enum: ["protect", "unlock"], default: "protect" },
      userPassword: { type: "string", title: "Password to open", "x-when": { mode: "protect" } },
      confirmRights: { type: "boolean", title: "I have the right", "x-when": { mode: "unlock" } },
    },
  };
  const ctx = { files: [], pages: 0, reference: "", api: {} as Api };
  const render = (params: Record<string, unknown>) =>
    renderToStaticMarkup(<SchemaForm schema={schema} params={params} onChange={() => {}} ctx={ctx} invalid={[]} />);
  const protect = render({});
  expect(protect).toContain("Password to open");
  expect(protect).not.toContain("I have the right");
  const unlock = render({ mode: "unlock" });
  expect(unlock).toContain("I have the right");
  expect(unlock).not.toContain("Password to open");
});

test("x-when lists, labels and the hidden target field", () => {
  const schema = {
    type: "object",
    properties: {
      to: { enum: ["jpg", "png"] },
      quality: { type: "integer", title: "Quality", minimum: 10, maximum: 100, default: 85, "x-ui": "slider", "x-when": { to: ["jpg", "webp"] } },
      maxSide: { type: "integer", title: "Size", enum: [0, 512], default: 0, "x-labels": { "0": "Original size", "512": "Up to 512 px" } },
    },
  };
  const ctx = { files: [], pages: 0, reference: "", api: {} as Api };
  const render = (params: Record<string, unknown>) =>
    renderToStaticMarkup(<SchemaForm schema={schema} params={params} onChange={() => {}} ctx={ctx} invalid={[]} />);
  const jpg = render({ to: "jpg" });
  expect(jpg).toContain("Quality");
  expect(jpg).toContain('type="range"');
  expect(jpg).toContain("Original size");
  expect(jpg).not.toContain(">Convert to<");
  expect(render({ to: "png" })).not.toContain("Quality");
});

test("media facts read naturally", async () => {
  const { describeMedia } = await import("./formats");
  expect(describeMedia({ kind: "video", width: 1920, height: 1080, durationMs: 134_000, hasAudio: false })).toBe(
    "1920 × 1080 · 2:14 · no sound",
  );
  expect(describeMedia({ kind: "audio", durationMs: 3_725_000, sampleRate: 44100, channels: 2 })).toBe(
    "1:02:05 · 44.1 kHz · stereo",
  );
});

test("every catalog warning has text for people", async () => {
  const schema = (await import("../../../packages/protocol/schema/protocol.schema.json")).default as {
    definitions: { WarningCode: { enum: string[] } };
  };
  const { WARNINGS } = await import("./text");
  const missing = schema.definitions.WarningCode.enum.filter((c) => !(c in WARNINGS));
  expect(missing).toEqual([]);
});

test("the target format counts for Start", async () => {
  const { missingRequired } = await import("./features/SchemaForm");
  const schema = { type: "object", required: ["to"], properties: { to: { enum: ["jpg"] } } };
  expect(missingRequired(schema, {})).toEqual(["to"]);
  expect(missingRequired(schema, { to: "jpg" })).toEqual([]);
});

test("a mixed selection lists one adapter per kind of file", async () => {
  const { adaptersFor } = await import("./features/screens");
  const group = (format: string, adapter: string) => ({
    format,
    files: 1,
    conversions: [{ to: "pdf", tier: "H", plan: { hops: [{ adapter }] } }],
  });
  const t = {
    operations: [],
    groups: [group("docx", "office-convert"), group("jpg", "image-convert"), group("png", "image-convert")],
    shared: [{ to: "pdf", tier: "H" }],
  } as unknown as Parameters<typeof adaptersFor>[1];
  expect(adaptersFor("pdf", t)).toEqual(["office-convert", "image-convert"]);
  expect(adaptersFor("png", t)).toEqual([]);
});

test("a tool that saves in a chosen format shows the choice", async () => {
  const { defaults } = await import("./features/SchemaForm");
  const schema = {
    type: "object",
    required: ["to"],
    properties: {
      to: { title: "Save as", enum: ["mp3", "flac"], default: "mp3", "x-labels": { mp3: "MP3", flac: "FLAC" } },
      fileTitles: { type: "array" },
    },
  };
  const ctx = { files: [], pages: 0, reference: "", api: {} as Api };
  const form = renderToStaticMarkup(
    <SchemaForm schema={schema} params={{ to: "mp3" }} onChange={() => {}} ctx={ctx} invalid={[]} showTo />,
  );
  expect(form).toContain("Save as");
  expect(form).toContain("FLAC");
  expect(defaults(schema, true)).toEqual({ to: "mp3" });
  // A conversion's target is picked elsewhere, never in the form.
  expect(defaults(schema)).toEqual({});
});

test("the home screen lists tools for every kind of file", () => {
  const home = renderToStaticMarkup(<App />);
  for (const t of ["Compress", "Animated GIF", "Volume, fade and speed", "Save frames", "Combine", "Page numbers"]) {
    expect(home).toContain(t);
  }
});

test("pasted text from a spreadsheet becomes a table, other text stays text", async () => {
  const { looksTabular, readPaste } = await import("./App");
  expect(looksTabular("a\tb\n1\t2\n3\t4\n")).toBe(true);
  expect(looksTabular("a\tb")).toBe(false); // one line isn't a table
  expect(looksTabular("Dear Sam,\nthanks\tfor everything\nsee you")).toBe(false);
  const data = (text: string, files: File[] = []) =>
    ({ files, getData: (t: string) => (t === "text/plain" ? text : "") }) as unknown as DataTransfer;
  expect((await readPaste(data("x\ty\n1\t2")))?.kind).toBe("table");
  expect((await readPaste(data("# Notes")))?.kind).toBe("text");
  expect(await readPaste(data("   "))).toBeNull();
  const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "a.png", { type: "image/png" });
  const pasted = await readPaste(data("ignored", [png]));
  expect(pasted?.kind).toBe("image");
  expect(pasted?.bytes.length).toBe(4);
});
