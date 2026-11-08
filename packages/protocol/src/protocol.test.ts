import { readFileSync } from "node:fs";
import { Ajv } from "ajv";
import { describe, expect, it } from "vitest";
import { MAX_LINE_BYTES, PROTOCOL_VERSION } from "./index.ts";
import type { CoreMessage, WarningCode, WorkerMessage } from "./index.ts";

type Case = { name: string; direction: "core" | "worker"; message: Record<string, unknown> };

const read = (path: string) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const schema = read("../schema/protocol.schema.json");
const cases: { valid: Case[]; invalid: Case[] } = read("../fixtures/cases.json");

const ajv = new Ajv({ strict: true, allErrors: true, allowUnionTypes: true });
ajv.addFormat("password", true);
ajv.addSchema(schema, "protocol");
const validators = {
  core: ajv.compile({ $ref: "protocol#/definitions/CoreMessage" }),
  worker: ajv.compile({ $ref: "protocol#/definitions/WorkerMessage" }),
};

const messageTypes = (definition: string): string[] =>
  schema.definitions[definition].oneOf.map(
    (ref: { $ref: string }) =>
      schema.definitions[ref.$ref.split("/").pop()!].properties.type.const as string,
  );

describe("protocol schema", () => {
  it("compiles in strict mode", () => {
    expect(validators.core).toBeTypeOf("function");
  });

  it.each(cases.valid.map((c) => [c.name, c] as const))("accepts: %s", (_, c) => {
    const ok = validators[c.direction](c.message);
    expect(validators[c.direction].errors ?? []).toEqual([]);
    expect(ok).toBe(true);
  });

  it.each(cases.invalid.map((c) => [c.name, c] as const))("rejects: %s", (_, c) => {
    expect(validators[c.direction](c.message)).toBe(false);
  });

  it("has a valid fixture for every message type", () => {
    for (const direction of ["core", "worker"] as const) {
      const definition = direction === "core" ? "CoreMessage" : "WorkerMessage";
      const covered = new Set(
        cases.valid.filter((c) => c.direction === direction).map((c) => c.message.type),
      );
      for (const type of messageTypes(definition)) {
        expect(covered, `${direction} message '${type}'`).toContain(type);
      }
    }
  });

  it("keeps message types unique across both directions", () => {
    const all = [...messageTypes("CoreMessage"), ...messageTypes("WorkerMessage")];
    expect(new Set(all).size).toBe(all.length);
  });

  it("matches the exported constants", () => {
    expect(schema.definitions.ProtocolVersion.const).toBe(PROTOCOL_VERSION);
    expect(MAX_LINE_BYTES).toBe(1_048_576);
  });

  it("generated types accept the spec's example messages", () => {
    // Type-level check: these assignments must compile.
    const execute: CoreMessage = {
      v: 1,
      type: "execute",
      jobId: "j_7f3a",
      inputs: ["in/0.pdf"],
      outDir: "out",
      workDir: "work",
      params: {},
    };
    const result: WorkerMessage = {
      v: 1,
      type: "executeResult",
      jobId: "j_7f3a",
      outputs: [{ path: "out/0.pdf", suggestedName: "a.pdf", sizeBytes: 1, sha256: "0".repeat(64) }],
      fidelity: "E",
    };
    const code: WarningCode = "FORMS_FLATTENED";
    expect([execute.type, result.type, code]).toHaveLength(3);
  });
});
