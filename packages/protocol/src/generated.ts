// Generated from packages/protocol/schema/protocol.schema.json by packages/protocol/scripts/generate.mjs.
// Do not edit by hand. Run `pnpm generate` instead.

/**
 * A message the core sends to a worker on its stdin.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "CoreMessage".
 */
export type CoreMessage = Inspect | Validate | Estimate | Execute | Render | PageText | Cancel;
/**
 * The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5).
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "ProtocolVersion".
 */
export type ProtocolVersion = 1;
/**
 * Opaque job identifier chosen by the core.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "JobId".
 */
export type JobId = string;
/**
 * A path relative to the job directory, using forward slashes. No absolute paths, drive letters, backslashes, empty segments, or segments starting with a dot (so no '.' or '..'). The core still resolves and checks every path on disk.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "JobPath".
 */
export type JobPath = string;
/**
 * 1-based page number.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "PageNumber".
 */
export type PageNumber = number;
/**
 * A JobPath that must be inside out/.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "OutputPath".
 */
export type OutputPath = string;
/**
 * A message a worker sends to the core on its protocol channel. Everything in it is untrusted (spec section 7.4).
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "WorkerMessage".
 */
export type WorkerMessage =
  | Hello
  | Stage
  | Progress
  | WorkerWarning
  | WorkerError
  | InspectResult
  | ValidateResult
  | EstimateResult
  | ExecuteResult
  | RenderResult
  | PageTextResult;
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "AdapterId".
 */
export type AdapterId = string;
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "StageName".
 */
export type StageName = string;
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Count".
 */
export type Count = number;
/**
 * A warning code as sent on the wire. Unknown codes are allowed here and handled by the core.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "WarningCodeString".
 */
export type WarningCodeString = string;
/**
 * Error codes in the catalog (spec section 13.1). A worker sending any other code fails the schema, which the core treats as E_PROTOCOL.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "ErrorCode".
 */
export type ErrorCode =
  | "E_INPUT_CORRUPT"
  | "E_PASSWORD_REQUIRED"
  | "E_PASSWORD_WRONG"
  | "E_LIMIT_EXCEEDED"
  | "E_RESOURCE_MEMORY"
  | "E_RESOURCE_DISK"
  | "E_TIMEOUT"
  | "E_CANCELLED"
  | "E_PROTOCOL"
  | "E_OUTPUT_INVALID"
  | "E_SANDBOX_UNAVAILABLE"
  | "E_INSTALL_CORRUPT"
  | "E_REDACTION_VERIFICATION_FAILED"
  | "E_INTERNAL";
/**
 * Authoritative encryption state from inspect: none, restrictions only (opens without a password), or an open password.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Encryption".
 */
export type Encryption = "none" | "permissionsOnly" | "openPassword";
/**
 * Lowercase hex SHA-256 digest.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Sha256".
 */
export type Sha256 = string;
/**
 * Fidelity tier (blueprint section 3.1): Exact, High, Reconstructed, Lossy.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Fidelity".
 */
export type Fidelity = "E" | "H" | "R" | "L";
/**
 * Warning codes in the catalog (spec section 13.1). Workers may send other codes; the core shows those as a generic warning, so the wire format accepts any WarningCodeString.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "WarningCode".
 */
export type WarningCode =
  | "FORMS_FLATTENED"
  | "XFA_UNSUPPORTED"
  | "SIGNATURE_INVALIDATED"
  | "TAGS_DROPPED"
  | "OUTLINE_DROPPED"
  | "ACTIVE_CONTENT_FOUND"
  | "NO_TEXT_LAYER"
  | "REDACTED_TEXT_REMAINS_ELSEWHERE"
  | "GLYPH_UNSUPPORTED"
  | "W_LARGE_JOB";

/**
 * One line of the worker protocol.
 */
export type Message = CoreMessage | WorkerMessage;
/**
 * Read-only facts about one input file.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Inspect".
 */
export interface Inspect {
  v: ProtocolVersion;
  type: "inspect";
  jobId: JobId;
  input: JobPath;
  secrets?: Secrets;
}
/**
 * Passwords for this request. This is the only place secrets may appear (spec section 7.3). Never log this object.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Secrets".
 */
export interface Secrets {
  /**
   * Password to open an encrypted input.
   */
  password?: string;
  /**
   * Open password to set on a protected output (pdf-protect).
   */
  userPassword?: string;
  /**
   * Owner password to set on a protected output (pdf-protect).
   */
  ownerPassword?: string;
}
/**
 * Adapter-specific checks of inputs and params.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Validate".
 */
export interface Validate {
  v: ProtocolVersion;
  type: "validate";
  jobId: JobId;
  /**
   * @minItems 1
   * @maxItems 1000
   */
  inputs: [JobPath, ...JobPath[]];
  params: Params;
  secrets?: Secrets;
}
/**
 * Adapter parameters. The core validates them against the adapter's own params.schema.json before sending.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Params".
 */
export interface Params {
  [k: string]: unknown | undefined;
}
/**
 * Resource estimate, using the metadata returned by validate.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Estimate".
 */
export interface Estimate {
  v: ProtocolVersion;
  type: "estimate";
  jobId: JobId;
  /**
   * @minItems 1
   * @maxItems 1000
   */
  inputs: [JobPath, ...JobPath[]];
  params: Params;
  metadata: Detail;
}
/**
 * Structured detail for a warning or error. Untrusted: render as plain text only, and length-limit it.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Detail".
 */
export interface Detail {
  [k: string]: unknown | undefined;
}
/**
 * Do the work. Outputs go only to outDir, scratch files only to workDir.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Execute".
 */
export interface Execute {
  v: ProtocolVersion;
  type: "execute";
  jobId: JobId;
  /**
   * @minItems 1
   * @maxItems 1000
   */
  inputs: [JobPath, ...JobPath[]];
  outDir: "out";
  workDir: "work";
  params: Params;
  secrets?: Secrets;
}
/**
 * Render one page to PNG for preview.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Render".
 */
export interface Render {
  v: ProtocolVersion;
  type: "render";
  jobId: JobId;
  input: JobPath;
  page: PageNumber;
  dpi: number;
  out: OutputPath;
  secrets?: Secrets;
}
/**
 * Word boxes on one page, for redaction search and display.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "PageText".
 */
export interface PageText {
  v: ProtocolVersion;
  type: "pageText";
  jobId: JobId;
  input: JobPath;
  page: PageNumber;
  secrets?: Secrets;
}
/**
 * Stop cooperatively. The worker answers with an E_CANCELLED error.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Cancel".
 */
export interface Cancel {
  v: ProtocolVersion;
  type: "cancel";
  jobId: JobId;
}
/**
 * Handshake, sent within 10 s of launch. v and protocolVersion are plain integers here, not the const, so that a mismatch is reported as E_INSTALL_CORRUPT rather than a schema failure.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Hello".
 */
export interface Hello {
  v: number;
  type: "hello";
  jobId: JobId;
  adapterId: AdapterId;
  protocolVersion: number;
  pid: number;
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Stage".
 */
export interface Stage {
  v: ProtocolVersion;
  type: "stage";
  jobId: JobId;
  name: StageName;
  index: number;
  of: number;
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "Progress".
 */
export interface Progress {
  v: ProtocolVersion;
  type: "progress";
  jobId: JobId;
  stage: StageName;
  done: Count;
  total?: Count;
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "WorkerWarning".
 */
export interface WorkerWarning {
  v: ProtocolVersion;
  type: "warning";
  jobId: JobId;
  code: WarningCodeString;
  detail?: Detail;
}
/**
 * The request failed. Ends the current request.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "WorkerError".
 */
export interface WorkerError {
  v: ProtocolVersion;
  type: "error";
  jobId: JobId;
  code: ErrorCode;
  detail?: Detail;
  retryable: boolean;
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "InspectResult".
 */
export interface InspectResult {
  v: ProtocolVersion;
  type: "inspectResult";
  jobId: JobId;
  pages: number;
  encryption: Encryption;
  signed?: boolean;
  /**
   * @maxItems 64
   */
  warnings?: WarningItem[];
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "WarningItem".
 */
export interface WarningItem {
  code: WarningCodeString;
  detail?: Detail;
}
/**
 * Validation outcome. Non-empty errors means the job must not run.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "ValidateResult".
 */
export interface ValidateResult {
  v: ProtocolVersion;
  type: "validateResult";
  jobId: JobId;
  /**
   * @maxItems 256
   */
  errors: ValidationIssue[];
  /**
   * @maxItems 256
   */
  warnings: WarningItem[];
  metadata: Detail;
}
export interface ValidationIssue {
  code: ErrorCode;
  input?: JobPath;
  param?: string;
  detail?: Detail;
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "EstimateResult".
 */
export interface EstimateResult {
  v: ProtocolVersion;
  type: "estimateResult";
  jobId: JobId;
  ramBytes: Count;
  diskBytes: Count;
  costUnits: Count;
}
/**
 * Success. The core re-checks every claim here against the files on disk.
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "ExecuteResult".
 */
export interface ExecuteResult {
  v: ProtocolVersion;
  type: "executeResult";
  jobId: JobId;
  /**
   * @minItems 1
   * @maxItems 10000
   */
  outputs: [ExecuteOutput, ...ExecuteOutput[]];
  fidelity: Fidelity;
}
export interface ExecuteOutput {
  path: OutputPath;
  /**
   * Untrusted. The core sanitizes it before use.
   */
  suggestedName: string;
  pages?: number;
  sizeBytes: Count;
  sha256: Sha256;
}
/**
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "RenderResult".
 */
export interface RenderResult {
  v: ProtocolVersion;
  type: "renderResult";
  jobId: JobId;
  out: OutputPath;
  widthPx: number;
  heightPx: number;
  sizeBytes: Count;
  sha256: Sha256;
}
/**
 * Word boxes in PDF points, origin top-left of the displayed page (after /Rotate and CropBox).
 *
 * This interface was referenced by `Message`'s JSON-Schema
 * via the `definition` "PageTextResult".
 */
export interface PageTextResult {
  v: ProtocolVersion;
  type: "pageTextResult";
  jobId: JobId;
  page: PageNumber;
  width: number;
  height: number;
  /**
   * @maxItems 20000
   */
  words: WordBox[];
}
export interface WordBox {
  text: string;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}
