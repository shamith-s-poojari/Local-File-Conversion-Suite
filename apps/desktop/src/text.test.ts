import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import * as text from "./text";
import { ERRORS, REASONS, REASONS_USING_CODE_TEXT } from "./text";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");

/** Source files under `dirs`, without build output, caches and tests. */
function sources(dirs: string[], ext: string): string[] {
  const out: string[] = [];
  const skip = new Set(["target", "node_modules", ".venv", "dist", "__pycache__", ".git", "tests", "gen"]);
  const walk = (dir: string) => {
    let names: string[];
    try {
      names = readdirSync(dir);
    } catch {
      return;
    }
    for (const name of names) {
      const path = join(dir, name);
      if (skip.has(name)) continue;
      const info = statSync(path);
      if (info.isDirectory()) walk(path);
      else if (name.endsWith(ext) && !name.startsWith("test_") && !name.endsWith("_test.py")) out.push(path);
    }
  };
  for (const d of dirs) walk(join(root, d));
  return out;
}

describe("error codes", () => {
  // Tests are left out on purpose: they invent codes ("E_FOO") to check how
  // an unknown one is handled, and those must not need text.
  const files = [
    ...sources(["crates", "apps"], ".rs"),
    ...sources(["workers", "adapters"], ".py"),
  ];

  test("the scan finds the sources it should", () => {
    const names = files.map((f) => relative(root, f).split(sep).join("/"));
    expect(names).toContain("crates/app-core/src/lib.rs");
    expect(names).toContain("workers/sdk/src/converter_sdk/errors.py");
    expect(names.some((n) => n.startsWith("adapters/"))).toBe(true);
    expect(names.some((n) => n.includes("/tests/"))).toBe(false);
  });

  test("every E_* code the Rust and Python sources use has text for people", () => {
    const codes = new Map<string, string>();
    for (const file of files) {
      for (const m of readFileSync(file, "utf8").matchAll(/"(E_[A-Z_]+)"/g)) {
        if (!codes.has(m[1]!)) codes.set(m[1]!, relative(root, file));
      }
    }
    expect(codes.size).toBeGreaterThan(10);
    const missing = [...codes].filter(([code]) => !(code in ERRORS)).map(([code, file]) => `${code} (${file})`);
    expect(missing).toEqual([]);
  });

  test("every text says what happened", () => {
    for (const [code, m] of Object.entries(ERRORS)) expect(m.what.length, code).toBeGreaterThan(5);
  });

  test("an unknown code falls back to a generic message", () => {
    expect(text.errorMessage("E_NOT_A_THING")).toBe(ERRORS.E_INTERNAL);
  });
});

describe("command reasons (U5)", () => {
  const source = readFileSync(join(root, "crates/app-core/src/lib.rs"), "utf8");
  // `Reason::AllSkipped => "allSkipped",` in `Reason::as_str`.
  const reasons = [...source.matchAll(/Reason::\w+ => "([a-zA-Z]+)",/g)].map((m) => m[1]!);

  test("the scan finds the Rust enum's reasons", () => {
    expect(reasons).toContain("allSkipped");
    expect(reasons).toContain("selfTestPending");
    expect(reasons.length).toBeGreaterThan(40);
  });

  test("every reason has text, or is listed as using its code's text", () => {
    const covered = new Set([...Object.keys(REASONS), ...REASONS_USING_CODE_TEXT]);
    expect(reasons.filter((r) => !covered.has(r))).toEqual([]);
  });

  test("no reason is both, and none is listed as code-text that the core doesn't give", () => {
    expect(REASONS_USING_CODE_TEXT.filter((r) => r in REASONS)).toEqual([]);
    const known = new Set(reasons);
    expect(REASONS_USING_CODE_TEXT.filter((r) => !known.has(r))).toEqual([]);
  });

  test("every reason text says what happened", () => {
    for (const [r, m] of Object.entries(REASONS)) expect(m.what.length, r).toBeGreaterThan(5);
  });
});

describe("text catalog", () => {
  test("the unused tier labels are gone; formats.ts owns them (U21)", () => {
    expect("TIER_LABELS" in text).toBe(false);
  });

  test("the limit text doesn't mention Settings (U4)", () => {
    expect(JSON.stringify(ERRORS.E_LIMIT_EXCEEDED)).not.toContain("Settings");
  });
});

describe("no command-line wording in the app's text (UX-11)", () => {
  test("no string in text.ts names a program, a flag or an environment variable", () => {
    const all = JSON.stringify(Object.values(text).filter((v) => typeof v !== "function"));
    expect(all).not.toMatch(/lfcs|LFCS_|--dev|\s--[a-z]/i);
    // The functions' output too.
    const fns = [text.installHint("ffmpeg", "macos"), text.folderTruncatedText(2), text.skippedText(1, "PDF"), text.REDACTION_LINES.added(1, 2)];
    expect(fns.join(" ")).not.toMatch(/lfcs|LFCS_|\s--[a-z]/i);
  });

  test("the codes the command line used to hint at now speak for the app", () => {
    expect(text.errorMessage("E_USAGE").todo).not.toContain("lfcs");
    expect(text.errorMessage("E_NO_ROUTE").todo).not.toContain("lfcs");
  });
});
