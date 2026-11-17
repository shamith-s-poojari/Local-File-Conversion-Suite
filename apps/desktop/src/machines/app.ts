// The UI state machine (spec 12.1, blueprint section 7), in XState v5.
//
// It holds view state only. Job state always comes from Rust: `job` events
// move the machine, and after a reload `RESTORE` rebuilds it from
// `get_snapshot`. Side effects are actors and actions with real
// implementations in `App.tsx`; tests provide fakes.

import { assign, fromPromise, setup } from "xstate";
import type {
  CommandError,
  FileDescriptor,
  Format,
  FileResult,
  InspectInfo,
  JobEvent,
  JobSnapshot,
  JobState,
  SaveResult,
  Warning,
} from "../api";

export interface FileView {
  /** Present when the sniffer accepted the file. */
  file?: FileDescriptor;
  displayName: string;
  /** Sniffer or inspect error. */
  error?: string;
  /** The command's reason for `error`, when it gave one. */
  reason?: string;
  inspect?: InspectInfo;
  /** Why the user must act before continuing. */
  needs?: "password" | "format" | "download";
  /** For `folderTruncated`: how many files were left out. */
  count?: number;
}

export interface JobView {
  jobId: string;
  state: JobState;
  stage?: string;
  progress?: { done: number; total?: number };
  stages: string[];
  warnings: Warning[];
  error?: { code: string; retryable: boolean };
  outputs: JobSnapshot["outputs"];
}

export interface Context {
  files: FileView[];
  /** The adapter that will run: an operation, or a conversion's adapter. */
  operation: string | null;
  /** Set for conversions: the format to make (ADR 0003). */
  to: Format | null;
  params: Record<string, unknown>;
  showBestEffort: boolean;
  job: JobView | null;
  /** The last command error, shown on the current screen. */
  error: CommandError | null;
  save: SaveResult | null;
  /** Job IDs in `seq` order are applied once each. */
  lastSeq: number;
  /** A line to show over the current screen. `finishFirst`: files were
   * dropped while a job was in progress, so they were released (audit U2). */
  notice: "finishFirst" | null;
  /** How many files the last conversion left out as already in the target
   * format (audit U22). */
  skipped: number;
}

export type Event =
  | { type: "FILES_ADDED"; results: FileResult[] }
  | { type: "REMOVE_FILE"; fileId: string }
  | { type: "RESOLVED" }
  | { type: "ABANDON" }
  | { type: "ACK" }
  | { type: "SELECT_OPERATION"; operation: string; params: Record<string, unknown> }
  | { type: "SELECT_CONVERSION"; to: Format; adapter: string; params: Record<string, unknown> }
  | { type: "SET_PARAMS"; params: Record<string, unknown> }
  | { type: "TOGGLE_BEST_EFFORT" }
  | { type: "START" }
  | { type: "CLEAR" }
  | { type: "CANCEL" }
  | { type: "JOB"; event: JobEvent }
  | { type: "RETRY" }
  | { type: "DISMISS" }
  | { type: "EDIT" }
  | { type: "ADJUST" }
  | { type: "SAVE"; keepLayout?: boolean }
  | { type: "CONFIRM_OVERWRITE" }
  | { type: "DECLINE_OVERWRITE" }
  | { type: "DISCARD_CONFIRMED" }
  | { type: "NEW" }
  | { type: "DISMISS_NOTICE" }
  | { type: "RESTORE"; job: JobSnapshot; seq: number };

const ACTIVE: JobState[] = ["created", "staging", "validating", "queued"];
const RUNNING: JobState[] = ["running", "verifying"];

/** What creating a job gives back. */
export interface Started {
  jobId: string;
  /** Files left out because they were already in the target format. */
  skipped: number;
}

function fileIds(ctx: Context): string[] {
  return ctx.files.flatMap((f) => (f.file?.actionable && !f.needs ? [f.file.fileId] : []));
}

function toView(job: JobSnapshot): JobView {
  return {
    jobId: job.jobId,
    state: job.state,
    stage: job.stage,
    progress: job.progress,
    stages: job.stage ? [job.stage] : [],
    warnings: job.warnings,
    error: job.error,
    outputs: job.outputs,
  };
}

/** Is this event about our job and new? */
function ours(ctx: Context, e: JobEvent): boolean {
  return ctx.job?.jobId === e.jobId && e.seq > ctx.lastSeq;
}

function jobIs(states: JobState[]) {
  return ({ context, event }: { context: Context; event: Event }) =>
    event.type === "JOB" && ours(context, event.event) && states.includes(event.event.state);
}

export const appMachine = setup({
  types: {
    context: {} as Context,
    events: {} as Event,
  },
  actors: {
    inspectAll: fromPromise<FileView[], { files: FileView[] }>(async ({ input }) => input.files),
    createJob: fromPromise<
      Started,
      { operation: string; to: Format | null; fileIds: string[]; params: Record<string, unknown> }
    >(async () => {
      throw new Error("not provided");
    }),
    save: fromPromise<SaveResult, { jobId: string; confirm: boolean; keepLayout?: boolean; token?: string }>(async () => {
      throw new Error("not provided");
    }),
    snapshotOutputs: fromPromise<JobSnapshot["outputs"], { jobId: string }>(async () => []),
  },
  actions: {
    // Side effects: real implementations are provided by App.tsx.
    cancelJob: (_, _params: { jobId: string }) => {},
    discardJob: (_, _params: { jobId: string }) => {},
    cancelSave: (_, _params: { jobId: string; token: string }) => {},
    removeFile: (_, _params: { fileId: string }) => {},
    clearFiles: (_, _params: { fileIds: string[] }) => {},
    applyJob: assign(({ context, event }) => {
      if (event.type !== "JOB" || !context.job || !ours(context, event.event)) return {};
      const e = event.event;
      const last = context.job.stages[context.job.stages.length - 1];
      const stages = e.stage && last !== e.stage ? [...context.job.stages, e.stage] : context.job.stages;
      return {
        lastSeq: e.seq,
        job: {
          ...context.job,
          state: e.state,
          stage: e.stage ?? context.job.stage,
          progress: e.progress ?? context.job.progress,
          stages,
          warnings: [...context.job.warnings, ...(e.warnings ?? [])],
          error: e.error ?? context.job.error,
        },
      };
    }),
    restore: assign(({ event }) =>
      event.type === "RESTORE" ? { job: toView(event.job), lastSeq: event.seq } : {},
    ),
  },
  guards: {
    allInvalid: ({ event }) =>
      "output" in event &&
      (event.output as FileView[]).every((f) => !f.file?.actionable && !f.needs),
    needsInput: ({ event }) =>
      "output" in event && (event.output as FileView[]).some((f) => f.needs),
    canStart: ({ context }) => context.operation !== null && fileIds(context).length > 0,
    queued: jobIs(["queued"]),
    running: jobIs(RUNNING),
    done: jobIs(["succeeded", "partial"]),
    failed: jobIs(["failed"]),
    cancelled: jobIs(["cancelled"]),
    restoreQueued: ({ event }) => event.type === "RESTORE" && ACTIVE.includes(event.job.state),
    restoreRunning: ({ event }) => event.type === "RESTORE" && RUNNING.includes(event.job.state),
    restoreDone: ({ event }) =>
      event.type === "RESTORE" && ["succeeded", "partial"].includes(event.job.state),
    restoreFailed: ({ event }) => event.type === "RESTORE" && event.job.state === "failed",
    restoreCancelled: ({ event }) => event.type === "RESTORE" && event.job.state === "cancelled",
  },
}).createMachine({
  id: "app",
  initial: "idle",
  context: {
    files: [],
    operation: null,
    to: null,
    params: {},
    showBestEffort: false,
    job: null,
    error: null,
    save: null,
    lastSeq: 0,
    notice: null,
    skipped: 0,
  },
  on: {
    DISMISS_NOTICE: { actions: assign({ notice: null }) },
    // A drop in a state that can't take files: tell the user, and let Rust
    // forget the files, which it already holds (audit U2). States that can
    // take files override this.
    FILES_ADDED: {
      actions: [
        { type: "clearFiles", params: ({ event }) => ({ fileIds: resultIds(event.results) }) },
        assign({ notice: "finishFirst" as const }),
      ],
    },
    RESTORE: [
      { guard: "restoreQueued", target: ".queued", actions: "restore" },
      { guard: "restoreRunning", target: ".processing", actions: "restore" },
      { guard: "restoreDone", target: ".preview", actions: "restore" },
      { guard: "restoreFailed", target: ".failed", actions: "restore" },
      { guard: "restoreCancelled", target: ".cancelled", actions: "restore" },
    ],
  },
  states: {
    idle: {
      entry: assign({
        files: [],
        operation: null,
        to: null,
        params: {},
        job: null,
        error: null,
        save: null,
        notice: null,
        skipped: 0,
      }),
      on: {
        FILES_ADDED: {
          target: "inspecting",
          actions: assign({
            files: ({ event }) => event.results.map(fromResult),
          }),
        },
      },
    },
    inspecting: {
      on: {
        // Start again with everything: files already inspected are skipped.
        FILES_ADDED: {
          target: "inspecting",
          reenter: true,
          actions: assign({
            files: ({ context, event }) => [...context.files, ...event.results.map(fromResult)],
          }),
        },
      },
      invoke: {
        src: "inspectAll",
        input: ({ context }) => ({ files: context.files }),
        onDone: [
          {
            guard: "allInvalid",
            target: "rejected",
            actions: assign({ files: ({ event }) => event.output }),
          },
          {
            guard: "needsInput",
            target: "needsInput",
            actions: assign({ files: ({ event }) => event.output }),
          },
          { target: "configuring", actions: assign({ files: ({ event }) => event.output }) },
        ],
        onError: {
          target: "rejected",
          actions: assign({ error: ({ event }) => asError(event.error) }),
        },
      },
    },
    needsInput: {
      on: {
        RESOLVED: "inspecting",
        FILES_ADDED: {
          target: "inspecting",
          actions: assign({
            files: ({ context, event }) => [...context.files, ...event.results.map(fromResult)],
          }),
        },
        REMOVE_FILE: [
          {
            guard: nothingLeftAfterRemoving,
            target: "idle",
            actions: { type: "removeFile", params: ({ event }) => ({ fileId: event.fileId }) },
          },
          {
            target: "inspecting",
            actions: [
              { type: "removeFile", params: ({ event }) => ({ fileId: event.fileId }) },
              assign({
                files: ({ context, event }) =>
                  context.files.filter((f) => f.file?.fileId !== event.fileId),
              }),
            ],
          },
        ],
        ABANDON: {
          target: "idle",
          actions: { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
        },
      },
    },
    rejected: {
      on: {
        FILES_ADDED: {
          target: "inspecting",
          actions: assign({
            files: ({ context, event }) => [...context.files, ...event.results.map(fromResult)],
          }),
        },
        ACK: {
          target: "idle",
          actions: { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
        },
      },
    },
    configuring: {
      on: {
        SELECT_OPERATION: {
          actions: assign({
            operation: ({ event }) => event.operation,
            to: null,
            params: ({ event }) => event.params,
            error: null,
          }),
        },
        SELECT_CONVERSION: {
          actions: assign({
            operation: ({ event }) => event.adapter,
            to: ({ event }) => event.to,
            params: ({ event }) => event.params,
            error: null,
          }),
        },
        SET_PARAMS: { actions: assign({ params: ({ event }) => event.params, error: null }) },
        TOGGLE_BEST_EFFORT: {
          actions: assign({ showBestEffort: ({ context }) => !context.showBestEffort }),
        },
        FILES_ADDED: {
          target: "inspecting",
          actions: assign({
            files: ({ context, event }) => [...context.files, ...event.results.map(fromResult)],
          }),
        },
        REMOVE_FILE: [
          {
            guard: nothingLeftAfterRemoving,
            target: "idle",
            actions: { type: "removeFile", params: ({ event }) => ({ fileId: event.fileId }) },
          },
          {
            actions: [
              { type: "removeFile", params: ({ event }) => ({ fileId: event.fileId }) },
              assign({
                files: ({ context, event }) =>
                  context.files.filter((f) => f.file?.fileId !== event.fileId),
              }),
            ],
          },
        ],
        START: { guard: "canStart", target: "preflight" },
        CLEAR: {
          target: "idle",
          actions: { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
        },
      },
    },
    preflight: {
      invoke: {
        src: "createJob",
        input: ({ context }) => ({
          operation: context.operation ?? "",
          to: context.to,
          fileIds: fileIds(context),
          params: withoutConfirmations(context.params),
        }),
        onDone: {
          target: "queued",
          actions: assign({
            params: ({ context }) => withoutSecrets(context.params),
            skipped: ({ event }) => event.output.skipped,
            job: ({ event }) => ({
              jobId: event.output.jobId,
              state: "created" as JobState,
              stages: [],
              warnings: [],
              outputs: [],
            }),
            error: null,
          }),
        },
        // Validation errors and INSUFFICIENT_RESOURCES go back to the form.
        // Passwords stay: nothing was created, and a typo elsewhere
        // shouldn't mean typing them (twice) again (audit U15).
        onError: {
          target: "configuring",
          actions: assign({
            error: ({ event }) => asError(event.error),
          }),
        },
      },
    },
    queued: {
      on: {
        JOB: [
          { guard: "running", target: "processing", actions: "applyJob" },
          { guard: "done", target: "preview", actions: "applyJob" },
          { guard: "failed", target: "failed", actions: "applyJob" },
          { guard: "cancelled", target: "cancelled", actions: "applyJob" },
          { actions: "applyJob" },
        ],
        CANCEL: { target: "cancelling", actions: { type: "cancelJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) } },
      },
    },
    processing: {
      on: {
        JOB: [
          { guard: "done", target: "preview", actions: "applyJob" },
          { guard: "failed", target: "failed", actions: "applyJob" },
          { guard: "cancelled", target: "cancelled", actions: "applyJob" },
          // Memory retry: running → queued is allowed and stays here.
          { actions: "applyJob" },
        ],
        CANCEL: { target: "cancelling", actions: { type: "cancelJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) } },
      },
    },
    cancelling: {
      on: {
        JOB: [
          { guard: "cancelled", target: "cancelled", actions: "applyJob" },
          { guard: "failed", target: "failed", actions: "applyJob" },
          // It finished before the cancel landed.
          { guard: "done", target: "preview", actions: "applyJob" },
          { actions: "applyJob" },
        ],
      },
    },
    cancelled: {
      on: {
        EDIT: { target: "configuring", actions: [{ type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) }, assign({ job: null })] },
      },
    },
    failed: {
      on: {
        RETRY: { target: "configuring", actions: [{ type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) }, assign({ job: null })] },
        DISMISS: {
          target: "idle",
          actions: [
            { type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) },
            { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
          ],
        },
      },
    },
    preview: {
      initial: "loading",
      states: {
        loading: {
          invoke: {
            src: "snapshotOutputs",
            input: ({ context }) => ({ jobId: context.job?.jobId ?? "" }),
            onDone: {
              target: "ready",
              actions: assign({
                job: ({ context, event }) =>
                  context.job ? { ...context.job, outputs: event.output } : null,
              }),
            },
            onError: "ready",
          },
        },
        ready: {
          on: {
            SAVE: "saving",
            CONFIRM_OVERWRITE: "confirming",
            // The held destination is dropped on both sides; the token is
            // used up either way, so it leaves the context too.
            DECLINE_OVERWRITE: {
              actions: [
                {
                  type: "cancelSave",
                  params: ({ context }) => ({
                    jobId: context.job?.jobId ?? "",
                    token: context.save?.confirmToken ?? "",
                  }),
                },
                assign({ save: null }),
              ],
            },
          },
        },
        saving: {
          invoke: {
            src: "save",
            input: ({ context, event }) => ({
              jobId: context.job?.jobId ?? "",
              confirm: false,
              keepLayout: event.type === "SAVE" && event.keepLayout === true,
            }),
            onDone: [
              {
                guard: ({ event }) => event.output.status === "saved",
                target: "#app.saved",
                actions: assign({ save: ({ event }) => event.output }),
              },
              { target: "ready", actions: assign({ save: ({ event }) => event.output }) },
            ],
            onError: {
              target: "ready",
              actions: assign({ error: ({ event }) => asError(event.error) }),
            },
          },
        },
        confirming: {
          invoke: {
            src: "save",
            input: ({ context }) => ({
              jobId: context.job?.jobId ?? "",
              confirm: true,
              token: context.save?.confirmToken ?? "",
            }),
            onDone: {
              target: "#app.saved",
              actions: assign({ save: ({ event }) => event.output }),
            },
            // The token is used up whether or not the save worked.
            onError: {
              target: "ready",
              actions: assign({ error: ({ event }) => asError(event.error), save: null }),
            },
          },
        },
      },
      on: {
        ADJUST: { target: "configuring", actions: [{ type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) }, assign({ job: null, save: null })] },
        DISCARD_CONFIRMED: {
          target: "idle",
          actions: [
            { type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) },
            assign({ save: null }),
            { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
          ],
        },
      },
    },
    saved: {
      on: {
        NEW: {
          target: "idle",
          actions: { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
        },
      },
    },
  },
});

/** Password-widget values: sent once with `create_job`, then forgotten
 * (spec 12.3: never retained in machine context after submission). */
export const SECRET_PARAMS = ["userPassword", "ownerPassword"];

/** The form's "type it again" field for a password: `userPasswordConfirm`.
 * It's checked in the form and never sent. */
export const CONFIRM_SUFFIX = "Confirm";

function isSecret(key: string): boolean {
  return SECRET_PARAMS.some((s) => key === s || key === s + CONFIRM_SUFFIX);
}

function withoutSecrets(params: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(params).filter(([k]) => !isSecret(k)));
}

/** What `create_job` gets: the params without the confirmation fields. */
function withoutConfirmations(params: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(params).filter(([k]) => !SECRET_PARAMS.some((s) => k === s + CONFIRM_SUFFIX)),
  );
}

function fromResult(r: FileResult): FileView {
  return { file: r.file, displayName: r.displayName, error: r.error, count: r.count };
}

function resultIds(results: FileResult[]): string[] {
  return results.flatMap((r) => (r.file ? [r.file.fileId] : []));
}

/** A file the user can still act on: one the app can use, or one waiting for
 * a password, a type or a download. Rows that only show an error don't count. */
function usable(f: FileView): boolean {
  return Boolean(f.file && (f.file.actionable || f.needs));
}

/** Removing this file leaves nothing to convert: back to the start. */
function nothingLeftAfterRemoving({ context, event }: { context: Context; event: Event }): boolean {
  if (event.type !== "REMOVE_FILE") return false;
  return !context.files.filter((f) => f.file?.fileId !== event.fileId).some(usable);
}

function allIds(ctx: Context): string[] {
  return ctx.files.flatMap((f) => (f.file ? [f.file.fileId] : []));
}

export function asError(e: unknown): CommandError {
  if (typeof e === "object" && e !== null && "code" in e) return e as CommandError;
  return { code: "E_INTERNAL" };
}

export { fileIds };
