// Worker protocol types (spec section 7). The types come from generated.ts,
// which `pnpm generate` writes from schema/protocol.schema.json.

export type * from "./generated.ts";

/** Protocol version. Must equal `#/definitions/ProtocolVersion` in the schema. */
export const PROTOCOL_VERSION = 1;

/** Maximum length of one NDJSON line in bytes, excluding the newline (spec section 7.1). */
export const MAX_LINE_BYTES = 1024 * 1024;
