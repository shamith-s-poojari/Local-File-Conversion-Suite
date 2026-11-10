// Typed bindings for the app's Tauri commands and events (spec section 11).
// The renderer holds file IDs and job IDs only: never paths or file bytes.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Format =
  | "pdf" | "docx" | "pptx" | "xlsx" | "md" | "html" | "txt"
  | "png" | "jpg" | "webp" | "svg" | "heic" | "unknown";

export interface FileDescriptor {
  fileId: string;
  /** Untrusted: render as text, never as markup. */
  displayName: string;
  sizeBytes: number;
  format: Format;
  confidence: string;
  evidence: string[];
  extensionMismatch: boolean;
  candidates: Format[];
  encryptionHint: "none" | "maybe";
  macroEnabled: boolean;
  cloudPlaceholder: boolean;
  actionable: boolean;
}

export interface FileResult {
  file?: FileDescriptor;
  error?: string;
  displayName: string;
}

export interface InspectInfo {
  pages: number;
  encryption: "none" | "permissionsOnly" | "openPassword";
  signed: boolean | null;
  warnings: string[];
}

export interface CommandError {
  code: string;
  reason?: string;
  issues?: { path: string }[];
}

export type JobState =
  | "created" | "staging" | "validating" | "queued" | "running"
  | "verifying" | "succeeded" | "partial" | "failed" | "cancelled";

export interface Warning { code: string; detail?: unknown }
export interface Progress { done: number; total?: number }

export interface JobEvent {
  seq: number;
  jobId: string;
  state: JobState;
  stage?: string;
  progress?: Progress;
  warnings?: Warning[];
  error?: { code: string; retryable: boolean };
}

export interface OutputFile {
  path: string;
  suggestedName: string;
  extension: string;
  sizeBytes: number;
}

export interface JobSnapshot {
  jobId: string;
  state: JobState;
  stage?: string;
  progress?: Progress;
  warnings: Warning[];
  error?: { code: string; retryable: boolean };
  outputs: OutputFile[];
}

export interface Snapshot { seq: number; jobs: JobSnapshot[] }

export interface OperationTarget {
  adapter: string;
  title: string;
  tier: "E" | "H" | "R" | "L";
  bestEffort: boolean;
  unavailable?: unknown;
}

export interface Targets { operations: OperationTarget[] }

export interface JsonSchema {
  type?: string | string[];
  title?: string;
  description?: string;
  enum?: unknown[];
  default?: unknown;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  items?: JsonSchema;
  readOnly?: boolean;
  writeOnly?: boolean;
  "x-ui"?: string;
}

export interface AdapterEntry {
  manifest: { id: string; title: string; inputs?: { min: number; max: number } };
  paramsSchema: JsonSchema;
}

export interface Registry { adapters: AdapterEntry[] }

export interface Preview { path: string; widthPx: number; heightPx: number }
export interface WordBox { text: string; x0: number; y0: number; x1: number; y1: number }
export interface PageText { width: number; height: number; words: WordBox[] }

export interface ProbeReport {
  attempts: { name: string; expected: string; outcome: string; passed: boolean }[];
  passed: boolean;
}

export interface Diagnostics {
  appVersion?: string;
  protocolVersion?: number;
  adapters?: string[];
  sandbox?: ProbeReport | null;
  processingEnabled: boolean;
  startupError?: string;
}

export interface SaveResult {
  status: "saved" | "cancelled" | "confirmOverwrite";
  count: number;
  syncProvider?: string | null;
}

/** The commands, one function each. Errors reject with a `CommandError`. */
export const api = {
  pickFiles: () => invoke<FileResult[]>("pick_files"),
  removeFile: (fileId: string) => invoke<void>("remove_file", { fileId }),
  resolveFile: (fileId: string, format?: Format, allowDownload?: boolean) =>
    invoke<FileDescriptor>("resolve_file", { fileId, format, allowDownload }),
  submitPassword: (fileId: string, password: string) =>
    invoke<InspectInfo>("submit_password", { fileId, password }),
  inspectFile: (fileId: string) => invoke<InspectInfo>("inspect_file", { fileId }),
  getTargets: (fileIds: string[]) => invoke<Targets>("get_targets", { fileIds }),
  getRegistry: () => invoke<Registry>("get_registry"),
  createJob: (operationId: string, fileIds: string[], params: Record<string, unknown>) =>
    invoke<string>("create_job", { operationId, fileIds, params }),
  cancelJob: (jobId: string) => invoke<boolean>("cancel_job", { jobId }),
  requestPreview: (reference: string, page: number, dpi: number) =>
    invoke<Preview>("request_preview", { reference, page, dpi }),
  getPageText: (reference: string, page: number) =>
    invoke<PageText>("get_page_text", { reference, page }),
  saveOutputs: (jobId: string) => invoke<SaveResult>("save_outputs", { jobId }),
  confirmSave: (jobId: string) => invoke<SaveResult>("confirm_save", { jobId }),
  discardJob: (jobId: string) => invoke<void>("discard_job", { jobId }),
  getSnapshot: () => invoke<Snapshot>("get_snapshot"),
  getDiagnostics: () => invoke<Diagnostics>("get_diagnostics"),
};

export type Api = typeof api;

export const events = {
  onJob: (f: (e: JobEvent) => void): Promise<UnlistenFn> =>
    listen<JobEvent>("job", (e) => f(e.payload)),
  onFilesAdded: (f: (files: FileResult[]) => void): Promise<UnlistenFn> =>
    listen<FileResult[]>("files-added", (e) => f(e.payload)),
  onSelfTest: (f: (r: ProbeReport) => void): Promise<UnlistenFn> =>
    listen<ProbeReport>("selftest", (e) => f(e.payload)),
};

/** `preview://localhost/<path>`, or the form Windows WebView2 needs. */
export function previewUrl(path: string): string {
  const windows = typeof navigator !== "undefined" && /Windows/.test(navigator.userAgent);
  return windows ? `http://preview.localhost/${path}` : `preview://localhost/${path}`;
}

export function isCommandError(e: unknown): e is CommandError {
  return typeof e === "object" && e !== null && "code" in e;
}
