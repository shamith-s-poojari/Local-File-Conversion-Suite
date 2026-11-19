// Tests for the 2026-11-15 audit fixes in the UI state machine: U2, U22, U26.

import { describe, expect, test, vi } from "vitest";
import { createActor, fromPromise, waitFor } from "xstate";
import type { FileResult, JobEvent, OutputFile, SaveResult } from "../api";
import { appMachine, type FileView } from "./app";

const pdf = (id: string): FileResult => ({
  displayName: `${id}.pdf`,
  file: {
    fileId: id,
    displayName: `${id}.pdf`,
    sizeBytes: 10,
    format: "pdf",
    confidence: "certain",
    evidence: [],
    extensionMismatch: false,
    candidates: [],
    encryptionHint: "none",
    macroEnabled: false,
    cloudPlaceholder: false,
    actionable: true,
  },
});
const unreadable: FileResult = { displayName: "x.bin", error: "unreadable" };

interface Options {
  inspect?: (files: FileView[]) => FileView[] | Promise<FileView[]>;
  createJob?: () => Promise<{ jobId: string; skipped: number }>;
  save?: () => Promise<SaveResult>;
}

function start(opts: Options = {}) {
  const effects = { removeFile: vi.fn(), clearFiles: vi.fn() };
  const machine = appMachine.provide({
    actors: {
      inspectAll: fromPromise(async ({ input }: { input: { files: FileView[] } }) =>
        (opts.inspect ?? ((f) => f))(input.files),
      ),
      createJob: fromPromise(async () => (opts.createJob ?? (async () => ({ jobId: "j_1", skipped: 0 })))()),
      save: fromPromise(async () => (opts.save ?? (async () => ({ status: "saved", count: 1 }) as SaveResult))()),
      snapshotOutputs: fromPromise(async () => [] as OutputFile[]),
    },
    actions: {
      cancelJob: () => {},
      discardJob: () => {},
      removeFile: (_, p) => effects.removeFile(p.fileId),
      clearFiles: (_, p) => effects.clearFiles(p.fileIds),
    },
  });
  return { actor: createActor(machine).start(), effects };
}
type Started = ReturnType<typeof start>;

const at = (s: Started, state: string | Record<string, string>) =>
  waitFor(s.actor, (x) => x.matches(state as Parameters<typeof x.matches>[0]), { timeout: 1000 });

let seq = 1000;
const job = (state: JobEvent["state"], extra: Partial<JobEvent> = {}) =>
  ({ type: "JOB", event: { seq: ++seq, jobId: "j_1", state, ...extra } }) as const;

/** Every state a drop can arrive in, each reached the way a user would. */
const REACH: Record<string, () => Promise<Started>> = {
  idle: async () => start(),
  inspecting: async () => {
    // The first inspection never finishes; a second (after a drop) does.
    let calls = 0;
    const s = start({ inspect: (f) => (calls++ === 0 ? new Promise(() => {}) : f) });
    s.actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    return s;
  },
  needsInput: async () => {
    const s = start({ inspect: (fs) => fs.map((f) => ({ ...f, needs: "password" as const })) });
    s.actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(s, "needsInput");
    return s;
  },
  rejected: async () => {
    const s = start();
    s.actor.send({ type: "FILES_ADDED", results: [unreadable] });
    await at(s, "rejected");
    return s;
  },
  configuring: async () => {
    const s = start();
    s.actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(s, "configuring");
    return s;
  },
  preflight: async () => {
    const s = start({ createJob: () => new Promise(() => {}) });
    s.actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(s, "configuring");
    s.actor.send({ type: "SELECT_OPERATION", operation: "pdf-merge", params: {} });
    s.actor.send({ type: "START" });
    await at(s, "preflight");
    return s;
  },
  queued: async () => {
    const s = await REACH.configuring!();
    s.actor.send({ type: "SELECT_OPERATION", operation: "pdf-merge", params: {} });
    s.actor.send({ type: "START" });
    await at(s, "queued");
    return s;
  },
  processing: async () => {
    const s = await REACH.queued!();
    s.actor.send(job("running"));
    await at(s, "processing");
    return s;
  },
  cancelling: async () => {
    const s = await REACH.processing!();
    s.actor.send({ type: "CANCEL" });
    await at(s, "cancelling");
    return s;
  },
  cancelled: async () => {
    const s = await REACH.cancelling!();
    s.actor.send(job("cancelled"));
    await at(s, "cancelled");
    return s;
  },
  failed: async () => {
    const s = await REACH.processing!();
    s.actor.send(job("failed"));
    await at(s, "failed");
    return s;
  },
  preview: async () => {
    const s = await REACH.processing!();
    s.actor.send(job("succeeded"));
    await at(s, { preview: "ready" });
    return s;
  },
  saved: async () => {
    const s = await REACH.preview!();
    s.actor.send({ type: "SAVE" });
    await at(s, "saved");
    return s;
  },
};

describe("files dropped in any state (U2)", () => {
  test.each(Object.keys(REACH))("%s: the new file is merged in, or the user is told to finish first", async (name) => {
    const s = await REACH[name]!();
    const before = s.actor.getSnapshot().context.files.map((f) => f.file?.fileId).filter(Boolean);
    s.actor.send({ type: "FILES_ADDED", results: [pdf("z")] });
    // Let any re-inspection settle.
    await new Promise((r) => setTimeout(r, 20));
    const ctx = s.actor.getSnapshot().context;
    const ids = ctx.files.map((f) => f.file?.fileId);
    if (ids.includes("z")) {
      // Merged: nothing that was there is lost.
      for (const id of before) expect(ids).toContain(id);
    } else {
      // Not merged: the user is told, and .
      expect(ctx.notice).toBe("finishFirst");
      expect(s.effects.clearFiles).toHaveBeenCalledWith(["z"]);
    }
  });

  test("states that can't take files release them and say so, and the notice can be dismissed", async () => {
    for (const name of ["preflight", "queued", "processing", "cancelling", "cancelled", "failed", "preview", "saved"]) {
      const s = await REACH[name]!();
      s.actor.send({ type: "FILES_ADDED", results: [pdf("z"), pdf("y")] });
      const ctx = s.actor.getSnapshot().context;
      expect(ctx.notice, name).toBe("finishFirst");
      expect(ctx.files.map((f) => f.file?.fileId), name).not.toContain("z");
      expect(s.effects.clearFiles, name).toHaveBeenCalledWith(["z", "y"]);
      s.actor.send({ type: "DISMISS_NOTICE" });
      expect(s.actor.getSnapshot().context.notice, name).toBeNull();
    }
  });

  test("files dropped on the rejected, needs-input and inspecting screens join the others and are inspected", async () => {
    for (const name of ["inspecting", "needsInput", "rejected"]) {
      const s = await REACH[name]!();
      s.actor.send({ type: "FILES_ADDED", results: [pdf("z")] });
      await at(s, name === "inspecting" ? "configuring" : "inspecting");
      expect(s.actor.getSnapshot().context.files.some((f) => f.file?.fileId === "z"), name).toBe(true);
    }
  });

  test("a notice is cleared when the app returns to the start", async () => {
    const s = await REACH.failed!();
    s.actor.send({ type: "FILES_ADDED", results: [pdf("z")] });
    expect(s.actor.getSnapshot().context.notice).toBe("finishFirst");
    s.actor.send({ type: "DISMISS" });
    await at(s, "idle");
    expect(s.actor.getSnapshot().context.notice).toBeNull();
  });
});

describe("removing the last file (U26)", () => {
  test("from the needs-input screen goes to idle, not to an empty review", async () => {
    const s = await REACH.needsInput!();
    s.actor.send({ type: "REMOVE_FILE", fileId: "a" });
    await at(s, "idle");
    expect(s.effects.removeFile).toHaveBeenCalledWith("a");
  });

  test("leaving only an unreadable file behind counts as removing the last one", async () => {
    const s = start();
    s.actor.send({ type: "FILES_ADDED", results: [pdf("a"), unreadable] });
    await at(s, "configuring");
    s.actor.send({ type: "REMOVE_FILE", fileId: "a" });
    await at(s, "idle");
  });

  test("removing one of several still leaves the rest", async () => {
    const s = await REACH.configuring!();
    s.actor.send({ type: "FILES_ADDED", results: [pdf("b")] });
    await at(s, "configuring");
    s.actor.send({ type: "REMOVE_FILE", fileId: "a" });
    expect(s.actor.getSnapshot().value).toBe("configuring");
    expect(s.actor.getSnapshot().context.files.map((f) => f.file?.fileId)).toEqual(["b"]);
  });
});

describe("skipped files (U22)", () => {
  test("the number of files left out is kept for the result screen, and reset for the next job", async () => {
    const s = start({ createJob: async () => ({ jobId: "j_1", skipped: 2 }) });
    s.actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(s, "configuring");
    s.actor.send({ type: "SELECT_OPERATION", operation: "pdf-merge", params: {} });
    s.actor.send({ type: "START" });
    await at(s, "queued");
    expect(s.actor.getSnapshot().context.skipped).toBe(2);
    s.actor.send(job("failed"));
    await at(s, "failed");
    s.actor.send({ type: "DISMISS" });
    await at(s, "idle");
    expect(s.actor.getSnapshot().context.skipped).toBe(0);
  });
});
