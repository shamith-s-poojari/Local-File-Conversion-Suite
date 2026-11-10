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
        ? { pages: 0, encryption: "openPassword", signed: null, warnings: [] }
        : { pages: 3, encryption: "none", signed: false, warnings: [] },
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
