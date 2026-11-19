import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";
import { context } from "../fixtures";
import type { JobView } from "../machines/app";
import { CHECK_NAMES } from "../text";
import { ProgressScreen } from "./screens";
import { progressAnnouncement, stageLabel } from "./screens/Progress";

describe("what a screen reader is told about progress (U12)", () => {
  test("changes only when the stage changes or progress passes the next 10%", () => {
    const at = (pct: number | null) => progressAnnouncement("convert", pct);
    expect(at(41)).toBe(at(49));
    expect(at(49)).not.toBe(at(50));
    expect(at(3)).toBe(at(9));
    expect(at(null)).toBe("Converting");
    expect(progressAnnouncement("render", 41)).not.toBe(at(41));
    expect(at(100)).toContain("100%");
  });
  test("says Preparing before the first stage", () => {
    expect(progressAnnouncement(undefined, null)).toBe("Preparing");
  });
});

describe("stage names (U24)", () => {
  test("checks are named, and unknown ones keep their id", () => {
    expect(stageLabel("verified-V3")).toContain(CHECK_NAMES.V3);
    expect(stageLabel("verified-V99")).toContain("V99");
    expect(stageLabel("render")).toBe("Rendering pages");
  });
});

describe("ProgressScreen", () => {
  const running: JobView = {
    jobId: "j",
    state: "running",
    stage: "convert",
    progress: { done: 4, total: 10 },
    stages: ["convert"],
    warnings: [],
    outputs: [],
  };
  const m = renderToStaticMarkup(<ProgressScreen ctx={context({ job: running })} send={() => {}} cancelling={false} />);

  test("the elapsed time is not in a live region", () => {
    const live = m.match(/<[^>]*(?:aria-live|role="status")[^>]*>[\s\S]*?<\/(?:p|div|span)>/g) ?? [];
    expect(live.length).toBeGreaterThan(0);
    for (const region of live) expect(region).not.toMatch(/\b\d+s\b|\d+m \d\ds/);
    expect(m).toMatch(/\b0s\b/);
  });

  test("the live region says the stage and the 10% step", () => {
    expect(m).toMatch(/role="status"[^>]*>Converting, 40%/);
  });

  test("the bar and the heading are there; the heading takes focus", () => {
    expect(m).toContain('aria-valuenow="40"');
    expect(m).toMatch(/<h1[^>]*tabindex="-1"/);
    expect(m).toContain("Cancel");
  });
});
