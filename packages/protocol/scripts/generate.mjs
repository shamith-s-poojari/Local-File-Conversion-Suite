// Generates protocol code from schema/protocol.schema.json (spec section 7).
//
//   node scripts/generate.mjs           write the generated files
//   node scripts/generate.mjs --check   fail if any generated file is out of date
//
// Outputs:
//   packages/protocol/src/generated.ts                         TypeScript types
//   workers/sdk/src/converter_sdk/protocol/_generated.py       Pydantic models
// Rust types are generated at compile time by crates/protocol, so they can't drift.

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "json-schema-to-typescript";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(packageDir, "../..");
const schemaPath = join(packageDir, "schema", "protocol.schema.json");
const check = process.argv.includes("--check");

const HEADER =
  "Generated from packages/protocol/schema/protocol.schema.json by packages/protocol/scripts/generate.mjs.\n" +
  "Do not edit by hand. Run `pnpm generate` instead.";

async function typescript() {
  // json-schema-to-typescript only emits unreferenced definitions (such as
  // WarningCode) when the root is a plain object, so compile an object root
  // and then swap its empty interface for the real union.
  const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
  const root = schema.oneOf.map((ref) => ref.$ref.split("/").pop());
  delete schema.oneOf;
  delete schema.description;
  schema.type = "object";
  schema.additionalProperties = false;
  const body = await compile(schema, "Message", {
    bannerComment: HEADER.split("\n").map((line) => `// ${line}`).join("\n"),
    additionalProperties: false,
    unreachableDefinitions: true,
    strictIndexSignatures: true,
  });
  const placeholder = "export interface Message {}";
  if (!body.includes(placeholder)) {
    throw new Error("json-schema-to-typescript output changed; update the Message replacement");
  }
  return body.replace(
    placeholder,
    "/**\n * One line of the worker protocol.\n */\n" +
      `export type Message = ${root.join(" | ")};`,
  );
}

function python() {
  const scratch = mkdtempSync(join(tmpdir(), "protocol-gen-"));
  const out = join(scratch, "_generated.py");
  try {
    const result = spawnSync(
      "uv",
      [
        "run",
        "--frozen",
        "datamodel-codegen",
        "--input", schemaPath,
        "--input-file-type", "jsonschema",
        "--output", out,
        "--output-model-type", "pydantic_v2.BaseModel",
        "--target-python-version", "3.12",
        "--use-annotated",
        "--field-constraints",
        "--use-double-quotes",
        "--enum-field-as-literal", "all",
        "--use-standard-collections",
        "--use-union-operator",
        "--use-title-as-name",
        "--collapse-root-models",
        "--use-one-literal-as-default",
        "--disable-timestamp",
        "--formatters", "ruff-check", "ruff-format",
        "--custom-file-header", HEADER.split("\n").map((line) => `# ${line}`).join("\n"),
      ],
      { cwd: repoRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
    if (result.status !== 0) {
      throw new Error(`datamodel-codegen failed:\n${result.stderr || result.error}`);
    }
    return readFileSync(out, "utf8");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

const normalize = (text) => text.replace(/\r\n/g, "\n").trimEnd() + "\n";

const outputs = [
  [join(packageDir, "src", "generated.ts"), await typescript()],
  [join(repoRoot, "workers", "sdk", "src", "converter_sdk", "protocol", "_generated.py"), python()],
];

let stale = 0;
for (const [path, content] of outputs) {
  const wanted = normalize(content);
  const name = relative(repoRoot, path).replaceAll("\\", "/");
  if (check) {
    const current = existsSync(path) ? normalize(readFileSync(path, "utf8")) : null;
    if (current !== wanted) {
      console.error(`out of date: ${name}`);
      stale += 1;
    }
  } else {
    writeFileSync(path, wanted);
    console.log(`wrote ${name}`);
  }
}

if (stale > 0) {
  console.error("Generated protocol code is out of date. Run `pnpm generate` and commit the result.");
  process.exit(1);
}
