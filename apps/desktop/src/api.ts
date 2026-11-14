// Typed bindings for the app's Tauri commands and events (spec section 11).
// The renderer holds file IDs and job IDs only: never paths or file bytes.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Format =
  | "pdf" | "docx" | "doc" | "odt" | "rtf" | "pptx" | "ppt" | "odp"
  | "xlsx" | "xls" | "ods" | "csv" | "md" | "html" | "txt" | "epub" | "json" | "yaml"
  | "png" | "jpg" | "webp" | "svg" | "heic" | "gif" | "bmp" | "tiff" | "avif" | "ico"
  | "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "aiff"
  | "mp4" | "mov" | "mkv" | "webm" | "avi"
  | "unknown";

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
  /** For a file from a dropped folder: the folder's name, then the folders
   * below it, `/`-separated (`photos/2024/trip`). Never a path. */
  folder?: string;
}

export interface FileResult {
  file?: FileDescriptor;
  /** Also `emptyFolder` (a dropped folder had no usable files) and
   * `folderTruncated` (`count` files of dropped folders didn't fit under the
   * 200-file limit; `displayName` is the folder). */
  error?: string;
  displayName: string;
  count?: number;
}

export interface MediaInfo {
  kind: "image" | "audio" | "video" | "document";
  durationMs?: number;
  width?: number;
  height?: number;
  frames?: number;
  videoCodec?: string;
  audioCodec?: string;
  sampleRate?: number;
  channels?: number;
  bitrateKbps?: number;
  fps?: number;
  hasAudio?: boolean;
  hasVideo?: boolean;
}

export interface InspectInfo {
  pages: number;
  encryption: "none" | "permissionsOnly" | "openPassword";
  signed: boolean | null;
  warnings: string[];
  media?: MediaInfo;
  /** What the file reveals or could do when opened (ADR 0005). */
  findings: Finding[];
}

export type FindingKind =
  | "location"
  | "device"
  | "author"
  | "software"
  | "dates"
  | "comments"
  | "trackedChanges"
  | "hiddenContent"
  | "activeContent"
  | "macros"
  | "externalLinks"
  | "embeddedFiles"
  | "tags";

export interface Finding {
  kind: FindingKind;
  detail?: string;
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
  /** The input this output came from, for keeping a folder's layout. */
  source?: number;
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

export type Tier = "E" | "H" | "R" | "L";

export interface Hop { adapter: string; from: Format; to: Format; tier: Tier }

export interface ConversionTarget {
  to: Format;
  tier: Tier;
  bestEffort: boolean;
  exactWithTextLayer: boolean;
  plan?: { hops: Hop[]; tier: Tier; cost: number };
  unavailable?: unknown;
}

export interface FormatGroup { format: Format; files: number; conversions: ConversionTarget[] }

export interface Targets {
  operations: OperationTarget[];
  groups: FormatGroup[];
  /** Targets every selected format reaches with one adapter ("all to PDF"). */
  shared: ConversionTarget[];
}

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
  /** Display labels for enum values, keyed by the value as text. */
  "x-labels"?: Record<string, string>;
  /** Show this field only while other params have these values (a list
   * means any of them). */
  "x-when"?: Record<string, unknown>;
}

export interface AdapterEntry {
  manifest: {
    id: string;
    kind: "operation" | "conversion";
    title: string;
    inputs?: { formats: Format[]; min: number; max: number };
  };
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
  /** Engines found on this computer, e.g. "ffmpeg", "libreoffice". */
  engines?: string[];
  /** "macos", "windows" or "linux". */
  platform?: string;
}

export interface ConversionStarted {
  jobId: string;
  /** Files left out because they were already in the target format. */
  skipped: number;
}

export interface SaveResult {
  status: "saved" | "cancelled" | "confirmOverwrite";
  count: number;
  syncProvider?: string | null;
}

/** The commands, one function each. Errors reject with a `CommandError`. */
export const api = {
  pickFiles: () => invoke<FileResult[]>("pick_files"),
  /** Native folder dialog; adds the folder's files. */
  pickFolder: () => invoke<FileResult[]>("pick_folder"),
  /** Something pasted in the window; Rust checks the bytes (max 50 MB). */
  addPasted: (kind: "image" | "table" | "text", bytes: Uint8Array) =>
    invoke<FileResult[]>("add_pasted", bytes, { headers: { "x-paste-kind": kind } }),
  removeFile: (fileId: string) => invoke<void>("remove_file", { fileId }),
  resolveFile: (fileId: string, format?: Format, allowDownload?: boolean) =>
    invoke<FileDescriptor>("resolve_file", { fileId, format, allowDownload }),
  submitPassword: (fileId: string, password: string) =>
    invoke<InspectInfo>("submit_password", { fileId, password }),
  inspectFile: (fileId: string) => invoke<InspectInfo>("inspect_file", { fileId }),
  findDuplicates: (fileIds: string[]) => invoke<number>("find_duplicates", { fileIds }),
  getTargets: (fileIds: string[]) => invoke<Targets>("get_targets", { fileIds }),
  getRegistry: () => invoke<Registry>("get_registry"),
  createJob: (operationId: string, fileIds: string[], params: Record<string, unknown>) =>
    invoke<string>("create_job", { operationId, fileIds, params }),
  createConversion: (fileIds: string[], to: Format, params: Record<string, unknown>) =>
    invoke<string>("create_conversion", { fileIds, to, params }),
  /** Like `createConversion`; put `skipSame: true` in `params` to leave out
   * files already in the target format, and read how many were left out. */
  createConversionSkipping: (fileIds: string[], to: Format, params: Record<string, unknown>) =>
    invoke<ConversionStarted>("create_conversion_skipping", { fileIds, to, params }),
  cancelJob: (jobId: string) => invoke<boolean>("cancel_job", { jobId }),
  requestPreview: (reference: string, page: number, dpi: number) =>
    invoke<Preview>("request_preview", { reference, page, dpi }),
  getPageText: (reference: string, page: number) =>
    invoke<PageText>("get_page_text", { reference, page }),
  saveOutputs: (jobId: string, keepLayout = false) =>
    invoke<SaveResult>("save_outputs", { jobId, keepLayout }),
  /** Whether saving can recreate the dropped folder's layout. */
  canKeepLayout: (jobId: string) => invoke<boolean>("can_keep_layout", { jobId }),
  /** Copies one output to the clipboard (PNG as an image; txt, md, csv,
   * json, yaml, html as text). Rejects with reason `notCopyable` otherwise. */
  copyOutput: (jobId: string, index: number) =>
    invoke<void>("copy_output", { jobId, index }),
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
