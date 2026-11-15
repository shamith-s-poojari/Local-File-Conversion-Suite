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
  | { type: "DISCARD_CONFIRMED" }
  | { type: "NEW" }
  | { type: "RESTORE"; job: JobSnapshot; seq: number };

const ACTIVE: JobState[] = ["created", "staging", "validating", "queued"];
const RUNNING: JobState[] = ["running", "verifying"];

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
      string,
      { operation: string; to: Format | null; fileIds: string[]; params: Record<string, unknown> }
    >(async () => {
      throw new Error("not provided");
    }),
    save: fromPromise<SaveResult, { jobId: string; confirm: boolean; keepLayout?: boolean }>(async () => {
      throw new Error("not provided");
    }),
    snapshotOutputs: fromPromise<JobSnapshot["outputs"], { jobId: string }>(async () => []),
  },
  actions: {
    // Side effects: real implementations are provided by App.tsx.
    cancelJob: (_, _params: { jobId: string }) => {},
    discardJob: (_, _params: { jobId: string }) => {},
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
  },
  on: {
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
        REMOVE_FILE: {
          target: "inspecting",
          actions: [
            { type: "removeFile", params: ({ event }) => ({ fileId: event.fileId }) },
            assign({
              files: ({ context, event }) =>
                context.files.filter((f) => f.file?.fileId !== event.fileId),
            }),
          ],
        },
        ABANDON: {
          target: "idle",
          actions: { type: "clearFiles", params: ({ context }) => ({ fileIds: allIds(context) }) },
        },
      },
    },
    rejected: {
      on: {
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
            guard: ({ context, event }) =>
              context.files.filter((f) => f.file?.fileId !== event.fileId).length === 0,
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
          params: context.params,
        }),
        onDone: {
          target: "queued",
          actions: assign({
            params: ({ context }) => withoutSecrets(context.params),
            job: ({ event }) => ({
              jobId: event.output,
              state: "created" as JobState,
              stages: [],
              warnings: [],
              outputs: [],
            }),
            error: null,
          }),
        },
        // Validation errors and INSUFFICIENT_RESOURCES go back to the form.
        onError: {
          target: "configuring",
          actions: assign({
            params: ({ context }) => withoutSecrets(context.params),
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
            input: ({ context }) => ({ jobId: context.job?.jobId ?? "", confirm: true }),
            onDone: {
              target: "#app.saved",
              actions: assign({ save: ({ event }) => event.output }),
            },
            onError: {
              target: "ready",
              actions: assign({ error: ({ event }) => asError(event.error) }),
            },
          },
        },
      },
      on: {
        ADJUST: { target: "configuring", actions: [{ type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) }, assign({ job: null })] },
        DISCARD_CONFIRMED: {
          target: "idle",
          actions: [
            { type: "discardJob", params: ({ context }) => ({ jobId: context.job?.jobId ?? "" }) },
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

function withoutSecrets(params: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(params).filter(([k]) => !SECRET_PARAMS.includes(k)));
}

function fromResult(r: FileResult): FileView {
  return { file: r.file, displayName: r.displayName, error: r.error, count: r.count };
}

function allIds(ctx: Context): string[] {
  return ctx.files.flatMap((f) => (f.file ? [f.file.fileId] : []));
}

export function asError(e: unknown): CommandError {
  if (typeof e === "object" && e !== null && "code" in e) return e as CommandError;
  return { code: "E_INTERNAL" };
}

export { fileIds };
