// Shared helpers for pickers, targets, diagnostics and screens.

import { afterEach, describe, expect, test, vi } from "vitest";
import type { Api, Targets } from "../api";
import { diagnostics } from "../fixtures";
import { screenOf } from "../App";
import { copyToClipboard, diagnosticsText } from "./diagnostics";
import { runPick } from "./pickers";
import { loadTargets } from "./targets";

describe("pickers (U6)", () => {
  const results = [{ displayName: "a.pdf" }];

  test("sends what was picked", async () => {
    const sent: unknown[] = [];
    const api = { pickFiles: async () => results } as unknown as Api;
    expect(await runPick(api, "files", (e) => sent.push(e))).toBeNull();
    expect(sent).toEqual([{ type: "FILES_ADDED", results }]);
  });

  test("a cancelled dialog sends nothing", async () => {
    const sent: unknown[] = [];
    const api = { pickFolder: async () => [] } as unknown as Api;
    expect(await runPick(api, "folder", (e) => sent.push(e))).toBeNull();
    expect(sent).toEqual([]);
  });

  test("a failing dialog gives words for the screen instead of an unhandled rejection", async () => {
    const api = {
      pickFiles: async () => {
        throw { code: "E_BUSY" };
      },
      pickFolder: async () => {
        throw new Error("boom");
      },
    } as unknown as Api;
    const sent: unknown[] = [];
    expect(await runPick(api, "files", (e) => sent.push(e))).toContain("busy");
    expect(await runPick(api, "folder", (e) => sent.push(e))).toContain("unexpected");
    expect(sent).toEqual([]);
  });
});

describe("targets (U28)", () => {
  test("a failure comes back with the real reason, not an empty list", async () => {
    const api = {
      getTargets: async () => {
        throw { code: "E_SANDBOX_UNAVAILABLE" };
      },
    } as unknown as Api;
    const r = await loadTargets(api, ["a"]);
    expect(r.error).toContain("Processing is turned off");
    expect(r.targets).toEqual({ operations: [], groups: [], shared: [] });
  });

  test("success has no error", async () => {
    const t: Targets = { operations: [], groups: [], shared: [] };
    const r = await loadTargets({ getTargets: async () => t } as unknown as Api, ["a"]);
    expect(r).toEqual({ targets: t, error: null });
  });
});

describe("diagnostics (U30)", () => {
  afterEach(() => vi.unstubAllGlobals());

  test("the text names the version, platform, engines and failed checks, and no files", () => {
    const t = diagnosticsText(diagnostics);
    expect(t).toContain("0.1.0");
    expect(t).toContain("macos");
    expect(t).toContain("ffmpeg");
    expect(t).toContain("connect_external");
    expect(t).not.toContain("read_sentinel: ");
  });

  test("copying uses the clipboard when there is one", async () => {
    const written: string[] = [];
    vi.stubGlobal("navigator", { clipboard: { writeText: async (s: string) => void written.push(s) } });
    expect(await copyToClipboard("hello")).toBe(true);
    expect(written).toEqual(["hello"]);
  });

  test("copying says no when the clipboard is missing or refuses", async () => {
    vi.stubGlobal("navigator", {});
    expect(await copyToClipboard("x")).toBe(false);
    vi.stubGlobal("navigator", { clipboard: { writeText: async () => Promise.reject(new Error("denied")) } });
    expect(await copyToClipboard("x")).toBe(false);
  });
});

describe("screens (U11)", () => {
  test("related states share a screen, so focus moves only when the screen changes", () => {
    expect(screenOf("configuring", false)).toBe(screenOf("preflight", false));
    expect(screenOf("needsInput", false)).toBe(screenOf("rejected", false));
    expect(screenOf("queued", false)).toBe(screenOf("processing", false));
    expect(screenOf("cancelling", false)).toBe(screenOf("processing", false));
    expect(screenOf("preview", false)).not.toBe(screenOf("saved", false));
    expect(screenOf("idle", true)).toBe("settings");
    const names = ["idle", "inspecting", "configuring", "processing", "preview", "failed", "cancelled", "saved"].map((s) =>
      screenOf(s, false),
    );
    expect(new Set(names).size).toBe(names.length);
  });
});
