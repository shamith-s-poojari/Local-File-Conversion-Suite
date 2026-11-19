import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import type { Api, FileResult, InspectInfo } from "./api";
import { App, inspectAll } from "./App";
import { SchemaForm } from "./features/SchemaForm";
import { inputNames } from "./features/screens/Result";
import { partFailure } from "./features/screens/shared";

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

test("inspecting again skips files that were already inspected (U16)", async () => {
  const calls: string[] = [];
  const api = {
    inspectFile: async (id: string): Promise<InspectInfo> => {
      calls.push(id);
      return { pages: 3, encryption: "none", signed: false, warnings: [], findings: [] };
    },
  } as unknown as Api;
  const first = await inspectAll(api, [file("a"), file("b")].map((r) => ({ file: r.file, displayName: r.displayName })));
  expect(calls).toEqual(["a", "b"]);
  // A third file arrives: only it is opened in a sandbox.
  const second = await inspectAll(api, [...first, { file: file("c").file, displayName: "c.pdf" }]);
  expect(calls).toEqual(["a", "b", "c"]);
  expect(second.map((f) => f.inspect?.pages)).toEqual([3, 3, 3]);
});

test("a file that needed a password is inspected again once it has one (U16)", async () => {
  const calls: string[] = [];
  const api = {
    inspectFile: async (id: string): Promise<InspectInfo> => {
      calls.push(id);
      return { pages: 2, encryption: "none", signed: false, warnings: [], findings: [] };
    },
  } as unknown as Api;
  const [out] = await inspectAll(api, [{ file: file("p").file, displayName: "p.pdf", needs: "password" }]);
  expect(calls).toEqual(["p"]);
  expect(out?.inspect?.pages).toBe(2);
  expect(out?.needs).toBeUndefined();
});

test("an Office file's password isn't asked for: it can't be used, so the file shows why (U1)", async () => {
  const api = {
    inspectFile: async () => {
      throw { code: "E_PASSWORD_REQUIRED", reason: "officePassword" };
    },
  } as unknown as Api;
  const [out] = await inspectAll(api, [file("doc")]);
  expect(out?.needs).toBeUndefined();
  expect(out?.error).toBe("E_PASSWORD_REQUIRED");
  expect(out?.reason).toBe("officePassword");
});

test("a PDF's missing password still asks for it", async () => {
  const api = {
    inspectFile: async () => {
      throw { code: "E_PASSWORD_REQUIRED" };
    },
  } as unknown as Api;
  const [out] = await inspectAll(api, [file("pdf")]);
  expect(out?.needs).toBe("password");
});

// A mixed selection (a.docx, b.png, c.docx, d.png) is staged grouped by
// adapter: a, c, b, d. The core reports that order; the names follow it.
test("Partly done names the files in the core's staged order, not selection order", () => {
  const files = ["a.docx", "b.png", "c.docx", "d.png"].map((name, i) => ({
    displayName: name,
    file: { fileId: `f${i}` },
  })) as unknown as Parameters<typeof inputNames>[1];
  const staged = ["f0", "f2", "f1", "f3"];
  const names = inputNames(staged, files);
  expect(names).toEqual(["a.docx", "c.docx", "b.png", "d.png"]);
  // `detail.inputs` [2] is the third staged input: b.png.
  expect(partFailure({ code: "E_WORKER", inputs: [2] }, names)?.names).toBe("b.png");
});

test("Partly done falls back to File N while the order is unknown or a file is gone", () => {
  expect(inputNames(null, [])).toEqual([]);
  expect(partFailure({ code: "E_WORKER", inputs: [0, 1] }, inputNames(null, []))?.names).toBe("File 1, File 2");
  expect(inputNames(["gone"], [])).toEqual([""]);
});

test("a job that appears after Cancel is cancelled and discarded (UX-7)", async () => {
  const { startJob } = await import("./App");
  const calls: string[] = [];
  const api = {
    createJob: async () => "j_9",
    cancelJob: async (id: string) => void calls.push(`cancel ${id}`),
    discardJob: async (id: string) => void calls.push(`discard ${id}`),
  } as unknown as Api;
  const input = { operation: "pdf-merge", to: null, fileIds: ["a"], params: {} };
  const live = new AbortController();
  expect(await startJob(api, input, live.signal)).toEqual({ jobId: "j_9", skipped: 0 });
  expect(calls).toEqual([]);
  const stopped = new AbortController();
  stopped.abort();
  await startJob(api, input, stopped.signal);
  expect(calls).toEqual(["cancel j_9", "discard j_9"]);
});
