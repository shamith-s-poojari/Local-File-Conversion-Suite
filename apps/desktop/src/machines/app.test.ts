// Model tests for the UI state machine (spec 12.1): every transition, with
// fake actors in place of Tauri.

import { describe, expect, test, vi } from "vitest";
import { createActor, fromPromise, waitFor } from "xstate";
import type { FileDescriptor, FileResult, Format, JobEvent, JobSnapshot, SaveResult } from "../api";
import { appMachine, type FileView } from "./app";

const pdf = (id: string, extra: Partial<FileDescriptor> = {}): FileResult => ({
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
    ...extra,
  },
});
type CreateInput = { operation: string; to: Format | null; fileIds: string[]; params: Record<string, unknown> };
type Started = { jobId: string; skipped: number };
const bad: FileResult = { displayName: "x.bin", error: "unreadable" };

function start(
  opts: {
    inspect?: (files: FileView[]) => FileView[] | Promise<FileView[]>;
    createJob?: (input: CreateInput) => Promise<Started>;
    save?: (confirm: boolean, token?: string) => Promise<SaveResult>;
  } = {},
) {
  const effects = {
    cancelJob: vi.fn(),
    discardJob: vi.fn(),
    cancelSave: vi.fn(),
    removeFile: vi.fn(),
    clearFiles: vi.fn(),
  };
  const machine = appMachine.provide({
    actors: {
      inspectAll: fromPromise(async ({ input }: { input: { files: FileView[] } }) =>
        (opts.inspect ?? ((f) => f))(input.files),
      ),
      createJob: fromPromise(async ({ input }: { input: CreateInput }) =>
        (opts.createJob ?? (async () => ({ jobId: "j_1", skipped: 0 })))(input),
      ),
      save: fromPromise(async ({ input }: { input: { jobId: string; confirm: boolean; token?: string } }) =>
        (opts.save ?? (async () => ({ status: "saved", count: 1 }) as SaveResult))(input.confirm, input.token),
      ),
      snapshotOutputs: fromPromise(async () => [
        { path: "0.pdf", suggestedName: "merged.pdf", extension: "pdf", sizeBytes: 5 },
      ]),
    },
    actions: {
      cancelJob: (_, p) => effects.cancelJob(p.jobId),
      discardJob: (_, p) => effects.discardJob(p.jobId),
      cancelSave: (_, p) => effects.cancelSave(p.jobId, p.token),
      removeFile: (_, p) => effects.removeFile(p.fileId),
      clearFiles: (_, p) => effects.clearFiles(p.fileIds),
    },
  });
  const actor = createActor(machine).start();
  return { actor, effects };
}

const at = (actor: ReturnType<typeof start>["actor"], state: string | Record<string, string>) =>
  waitFor(actor, (s) => s.matches(state as Parameters<typeof s.matches>[0]), { timeout: 1000 });

let seq = 0;
const job = (state: JobEvent["state"], extra: Partial<JobEvent> = {}): { type: "JOB"; event: JobEvent } => ({
  type: "JOB",
  event: { seq: ++seq, jobId: "j_1", state, ...extra },
});

async function configured(opts: Parameters<typeof start>[0] = {}) {
  const s = start(opts);
  s.actor.send({ type: "FILES_ADDED", results: [pdf("a"), pdf("b")] });
  await at(s.actor, "configuring");
  s.actor.send({ type: "SELECT_OPERATION", operation: "pdf-merge", params: {} });
  return s;
}

async function running(opts: Parameters<typeof start>[0] = {}) {
  const s = await configured(opts);
  s.actor.send({ type: "START" });
  await at(s.actor, "queued");
  s.actor.send(job("running", { stage: "merge" }));
  await at(s.actor, "processing");
  return s;
}

describe("files", () => {
  test("idle → inspecting → configuring", async () => {
    const { actor } = start();
    expect(actor.getSnapshot().value).toBe("idle");
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(actor, "configuring");
  });

  test("all invalid → rejected → ACK → idle", async () => {
    const { actor, effects } = start();
    actor.send({ type: "FILES_ADDED", results: [bad] });
    await at(actor, "rejected");
    actor.send({ type: "ACK" });
    await at(actor, "idle");
    expect(effects.clearFiles).toHaveBeenCalled();
  });

  test("password needed → needsInput → RESOLVED → inspecting again", async () => {
    let calls = 0;
    const { actor } = start({
      inspect: (files) => files.map((f) => ({ ...f, needs: calls++ === 0 ? "password" : undefined })),
    });
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(actor, "needsInput");
    actor.send({ type: "RESOLVED" });
    await at(actor, "configuring");
  });

  test("needsInput: removing the file re-inspects, ABANDON goes idle", async () => {
    const { actor, effects } = start({
      inspect: (files) => files.map((f) => ({ ...f, needs: f.file?.fileId === "a" ? "password" : undefined })),
    });
    actor.send({ type: "FILES_ADDED", results: [pdf("a"), pdf("b")] });
    await at(actor, "needsInput");
    actor.send({ type: "REMOVE_FILE", fileId: "a" });
    await at(actor, "configuring");
    expect(effects.removeFile).toHaveBeenCalledWith("a");

    const second = start({ inspect: (f) => f.map((x) => ({ ...x, needs: "format" })) });
    second.actor.send({ type: "FILES_ADDED", results: [pdf("c")] });
    await at(second.actor, "needsInput");
    second.actor.send({ type: "ABANDON" });
    await at(second.actor, "idle");
  });

  test("inspection error → rejected with the error", async () => {
    const { actor } = start({
      inspect: () => {
        throw { code: "E_SANDBOX_UNAVAILABLE" };
      },
    });
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(actor, "rejected");
    expect(actor.getSnapshot().context.error?.code).toBe("E_SANDBOX_UNAVAILABLE");
  });
});

describe("configuring", () => {
  test("params, best-effort toggle, more files, remove, clear", async () => {
    const { actor, effects } = await configured();
    actor.send({ type: "SET_PARAMS", params: { bookmarks: "none" } });
    actor.send({ type: "TOGGLE_BEST_EFFORT" });
    const c = actor.getSnapshot().context;
    expect([c.params, c.showBestEffort]).toEqual([{ bookmarks: "none" }, true]);
    actor.send({ type: "FILES_ADDED", results: [pdf("c")] });
    await at(actor, "configuring");
    expect(actor.getSnapshot().context.files).toHaveLength(3);
    actor.send({ type: "REMOVE_FILE", fileId: "c" });
    expect(actor.getSnapshot().context.files).toHaveLength(2);
    actor.send({ type: "CLEAR" });
    await at(actor, "idle");
    expect(effects.clearFiles).toHaveBeenCalledWith(["a", "b"]);
  });

  test("removing the last file goes idle", async () => {
    const { actor } = start();
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(actor, "configuring");
    actor.send({ type: "REMOVE_FILE", fileId: "a" });
    await at(actor, "idle");
  });

  test("START needs an operation", async () => {
    const { actor } = start();
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await at(actor, "configuring");
    actor.send({ type: "START" });
    expect(actor.getSnapshot().value).toBe("configuring");
  });

  test("a password is forgotten once the job is created, and kept if creating it fails", async () => {
    // Failing to create a job (a wrong page range, say) shouldn't make the
    // user type the password, and its confirmation, again (audit U15).
    for (const fails of [false, true]) {
      const { actor } = await configured({
        createJob: async () => {
          if (fails) throw { code: "E_PARAMS_INVALID" };
          return { jobId: "j_1", skipped: 0 };
        },
      });
      actor.send({ type: "SET_PARAMS", params: { mode: "protect", userPassword: "s3cret", userPasswordConfirm: "s3cret" } });
      actor.send({ type: "START" });
      await at(actor, fails ? "configuring" : "queued");
      const { params } = actor.getSnapshot().context;
      if (fails) {
        expect(params).toEqual({ mode: "protect", userPassword: "s3cret", userPasswordConfirm: "s3cret" });
      } else {
        expect(params).toEqual({ mode: "protect" });
        expect(JSON.stringify(actor.getSnapshot().context)).not.toContain("s3cret");
      }
    }
  });

  test("the confirmation field is never sent with the job", async () => {
    const inputs: CreateInput[] = [];
    const { actor } = await configured({
      createJob: async (input) => {
        inputs.push(input);
        return { jobId: "j_1", skipped: 0 };
      },
    });
    actor.send({ type: "SET_PARAMS", params: { mode: "protect", userPassword: "pw", userPasswordConfirm: "pw" } });
    actor.send({ type: "START" });
    await at(actor, "queued");
    expect(inputs[0]?.params).toEqual({ mode: "protect", userPassword: "pw" });
  });

  test("preflight failure goes back to the form with the error", async () => {
    const { actor } = await configured({
      createJob: async () => {
        throw { code: "E_PARAMS_INVALID", issues: [{ path: "/mode" }] };
      },
    });
    actor.send({ type: "START" });
    await at(actor, "configuring");
    expect(actor.getSnapshot().context.error?.code).toBe("E_PARAMS_INVALID");
  });

  test("the reason of a preflight failure is kept, so every-file-skipped can say so (E1/U1)", async () => {
    const { actor } = await configured({
      createJob: async () => {
        throw { code: "E_PARAMS_INVALID", reason: "allSkipped" };
      },
    });
    actor.send({ type: "START" });
    await at(actor, "configuring");
    expect(actor.getSnapshot().context.error?.reason).toBe("allSkipped");
  });
});

describe("running", () => {
  test("queued → processing → preview, collecting stages and warnings", async () => {
    const { actor } = await running();
    actor.send(job("running", { stage: "write", progress: { done: 3, total: 4 } }));
    actor.send(job("verifying", { warnings: [{ code: "TAGS_DROPPED" }] }));
    actor.send(job("succeeded"));
    await at(actor, { preview: "ready" });
    const j = actor.getSnapshot().context.job!;
    expect(j.stages).toEqual(["merge", "write"]);
    expect(j.warnings.map((w) => w.code)).toEqual(["TAGS_DROPPED"]);
    expect(j.outputs[0]?.suggestedName).toBe("merged.pdf");
  });

  test("queued can finish, fail or be cancelled directly", async () => {
    for (const [state, target] of [
      ["succeeded", "preview"],
      ["failed", "failed"],
      ["cancelled", "cancelled"],
    ] as const) {
      const { actor } = await configured();
      actor.send({ type: "START" });
      await at(actor, "queued");
      actor.send(job(state));
      await at(actor, target);
    }
  });

  test("events for other jobs, or old events, are ignored", async () => {
    const { actor } = await running();
    actor.send({ type: "JOB", event: { seq: ++seq, jobId: "j_other", state: "failed" } });
    actor.send({ type: "JOB", event: { seq: 1, jobId: "j_1", state: "failed" } });
    expect(actor.getSnapshot().value).toBe("processing");
  });

  test("a memory retry (running → queued) stays in processing", async () => {
    const { actor } = await running();
    actor.send(job("queued"));
    expect(actor.getSnapshot().value).toBe("processing");
  });

  test("processing → failed → RETRY or DISMISS", async () => {
    const a = await running();
    a.actor.send(job("failed", { error: { code: "E_TIMEOUT", retryable: true } }));
    await at(a.actor, "failed");
    expect(a.actor.getSnapshot().context.job?.error?.code).toBe("E_TIMEOUT");
    a.actor.send({ type: "RETRY" });
    await at(a.actor, "configuring");
    expect(a.effects.discardJob).toHaveBeenCalledWith("j_1");

    const b = await running();
    b.actor.send(job("failed"));
    await at(b.actor, "failed");
    b.actor.send({ type: "DISMISS" });
    await at(b.actor, "idle");
  });

  test("CANCEL → cancelling → cancelled → EDIT", async () => {
    const { actor, effects } = await running();
    actor.send({ type: "CANCEL" });
    await at(actor, "cancelling");
    expect(effects.cancelJob).toHaveBeenCalledWith("j_1");
    actor.send(job("cancelled"));
    await at(actor, "cancelled");
    actor.send({ type: "EDIT" });
    await at(actor, "configuring");
  });

  test("cancel from queued, and a job that finishes before cancel lands", async () => {
    const a = await configured();
    a.actor.send({ type: "START" });
    await at(a.actor, "queued");
    a.actor.send({ type: "CANCEL" });
    await at(a.actor, "cancelling");
    a.actor.send(job("cancelled"));
    await at(a.actor, "cancelled");

    const b = await running();
    b.actor.send({ type: "CANCEL" });
    b.actor.send(job("succeeded"));
    await at(b.actor, "preview");

    const c = await running();
    c.actor.send({ type: "CANCEL" });
    c.actor.send(job("failed"));
    await at(c.actor, "failed");
  });
});

describe("result", () => {
  async function previewing(save?: (confirm: boolean, token?: string) => Promise<SaveResult>) {
    const s = await running({ save });
    s.actor.send(job("succeeded"));
    await at(s.actor, { preview: "ready" });
    return s;
  }

  test("SAVE → saved → NEW", async () => {
    const { actor } = await previewing();
    actor.send({ type: "SAVE" });
    await at(actor, "saved");
    actor.send({ type: "NEW" });
    await at(actor, "idle");
  });

  test("a cancelled dialog stays; a failed save shows the error", async () => {
    const a = await previewing(async () => ({ status: "cancelled", count: 0 }));
    a.actor.send({ type: "SAVE" });
    await at(a.actor, { preview: "ready" });

    const b = await previewing(async () => {
      throw { code: "E_RESOURCE_DISK" };
    });
    b.actor.send({ type: "SAVE" });
    await waitFor(b.actor, (s) => s.context.error?.code === "E_RESOURCE_DISK");
    expect(b.actor.getSnapshot().matches({ preview: "ready" })).toBe(true);
  });

  test("overwriting an input needs confirmation", async () => {
    const { actor } = await previewing(async (confirm) =>
      confirm ? { status: "saved", count: 1 } : { status: "confirmOverwrite", count: 0 },
    );
    actor.send({ type: "SAVE" });
    await waitFor(actor, (s) => s.context.save?.status === "confirmOverwrite");
    actor.send({ type: "CONFIRM_OVERWRITE" });
    await at(actor, "saved");
  });

  test("confirming sends the token the shell gave, and drops it", async () => {
    const tokens: (string | undefined)[] = [];
    const { actor } = await previewing(async (confirm, token) => {
      tokens.push(token);
      return confirm
        ? { status: "saved", count: 1 }
        : { status: "confirmOverwrite", count: 0, confirmToken: "t-1" };
    });
    actor.send({ type: "SAVE" });
    await waitFor(actor, (s) => s.context.save?.confirmToken === "t-1");
    actor.send({ type: "CONFIRM_OVERWRITE" });
    await at(actor, "saved");
    expect(tokens).toEqual([undefined, "t-1"]);
    expect(actor.getSnapshot().context.save?.confirmToken).toBeUndefined();
  });

  test("declining sends cancelSave with the token and forgets it", async () => {
    const { actor, effects } = await previewing(async () => ({
      status: "confirmOverwrite",
      count: 0,
      confirmToken: "t-2",
    }));
    actor.send({ type: "SAVE" });
    await waitFor(actor, (s) => s.context.save?.confirmToken === "t-2");
    actor.send({ type: "DECLINE_OVERWRITE" });
    expect(effects.cancelSave).toHaveBeenCalledWith("j_1", "t-2");
    expect(actor.getSnapshot().context.save).toBeNull();
    expect(actor.getSnapshot().matches({ preview: "ready" })).toBe(true);
  });

  test("a failed confirmation drops the used-up token", async () => {
    const { actor } = await previewing(async (confirm) => {
      if (confirm) throw { code: "E_INTERNAL", reason: "nothingPending" };
      return { status: "confirmOverwrite", count: 0, confirmToken: "t-3" };
    });
    actor.send({ type: "SAVE" });
    await waitFor(actor, (s) => s.context.save?.confirmToken === "t-3");
    actor.send({ type: "CONFIRM_OVERWRITE" });
    await waitFor(actor, (s) => s.context.error?.reason === "nothingPending");
    expect(actor.getSnapshot().context.save).toBeNull();
  });

  test("ADJUST and DISCARD_CONFIRMED", async () => {
    const a = await previewing();
    a.actor.send({ type: "ADJUST" });
    await at(a.actor, "configuring");
    expect(a.effects.discardJob).toHaveBeenCalled();

    const b = await previewing();
    b.actor.send({ type: "DISCARD_CONFIRMED" });
    await at(b.actor, "idle");
  });
});

describe("reload recovery", () => {
  const snap = (state: JobSnapshot["state"]): JobSnapshot => ({
    jobId: "j_9",
    state,
    warnings: [],
    outputs: [],
  });

  test.each([
    ["queued", "queued"],
    ["validating", "queued"],
    ["running", "processing"],
    ["succeeded", "preview"],
    ["failed", "failed"],
    ["cancelled", "cancelled"],
  ] as const)("a %s job restores to %s", async (state, target) => {
    const { actor } = start();
    actor.send({ type: "RESTORE", job: snap(state), seq: 40 });
    await at(actor, target);
    expect(actor.getSnapshot().context.job?.jobId).toBe("j_9");
    expect(actor.getSnapshot().context.lastSeq).toBe(40);
  });
});

describe("conversions (ADR 0003)", () => {
  test("choosing a format passes it to createJob, and choosing a tool clears it", async () => {
    const inputs: unknown[] = [];
    const machine = appMachine.provide({
      actors: {
        inspectAll: fromPromise(async ({ input }: { input: { files: FileView[] } }) => input.files),
        createJob: fromPromise(async ({ input }: { input: { operation: string; to: Format | null; fileIds: string[]; params: Record<string, unknown> } }) => {
          inputs.push(input);
          return { jobId: "j_9", skipped: 0 };
        }),
      },
    });
    const actor = createActor(machine).start();
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await waitFor(actor, (s) => s.matches("configuring"));
    actor.send({ type: "SELECT_CONVERSION", to: "png", adapter: "pdf-to-image", params: { dpi: 150 } });
    expect(actor.getSnapshot().context.to).toBe("png");
    expect(actor.getSnapshot().context.operation).toBe("pdf-to-image");
    actor.send({ type: "SELECT_OPERATION", operation: "pdf-split", params: {} });
    expect(actor.getSnapshot().context.to).toBeNull();
    actor.send({ type: "SELECT_CONVERSION", to: "jpg", adapter: "pdf-to-image", params: {} });
    actor.send({ type: "START" });
    await waitFor(actor, (s) => s.matches("queued"));
    expect(inputs[0]).toMatchObject({ operation: "pdf-to-image", to: "jpg", fileIds: ["a"] });
  });
});

describe("leaving while it works (UX-7)", () => {
  test("Cancel while files are being checked goes home and releases the files", async () => {
    const { actor, effects } = start({ inspect: () => new Promise(() => {}) });
    actor.send({ type: "FILES_ADDED", results: [pdf("a"), pdf("b")] });
    expect(actor.getSnapshot().value).toBe("inspecting");
    actor.send({ type: "CANCEL" });
    expect(actor.getSnapshot().value).toBe("idle");
    expect(effects.clearFiles).toHaveBeenCalledWith(["a", "b"]);
  });

  test("Cancel while the job is being set up goes back to the form, files and choices kept", async () => {
    const { actor } = start({ createJob: () => new Promise(() => {}) });
    actor.send({ type: "FILES_ADDED", results: [pdf("a")] });
    await waitFor(actor, (s) => s.matches("configuring"));
    actor.send({ type: "SELECT_OPERATION", operation: "pdf-compress", params: { level: 2 } });
    actor.send({ type: "START" });
    expect(actor.getSnapshot().value).toBe("preflight");
    actor.send({ type: "CANCEL" });
    const s = actor.getSnapshot();
    expect(s.value).toBe("configuring");
    expect(s.context.operation).toBe("pdf-compress");
    expect(s.context.files).toHaveLength(1);
  });
});

describe("Start over asks first only when work would be lost (UX-8)", () => {
  test("choices or several files need a question; one fresh file doesn't", async () => {
    const { startOverNeedsConfirm } = await import("./app");
    const { context } = await import("../fixtures");
    expect(startOverNeedsConfirm(context({ operation: "pdf-merge" }))).toBe(true);
    expect(startOverNeedsConfirm(context({ files: [context().files[0]!, context().files[0]!] }))).toBe(true);
    expect(startOverNeedsConfirm(context())).toBe(false);
  });
});
