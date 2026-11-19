import type { Diagnostics } from "../api";

/** Where the sandbox self-test stands, for the Home banner. While it runs the
 * core reports no result (`sandbox` is null) and processing is off, which is
 * not a failure and must not look like one (audit UX-5). */
export function selfTestState(diag: Diagnostics | null | undefined): "unknown" | "running" | "passed" | "failed" | "startupFailed" {
  if (!diag) return "unknown";
  if (diag.startupError) return "startupFailed";
  if (!diag.sandbox) return diag.processingEnabled ? "passed" : "running";
  return diag.processingEnabled ? "passed" : "failed";
}

/** The engines the app looks for that this computer doesn't have. */
export function missingEngines(diag: Diagnostics | null | undefined): ("ffmpeg" | "libreoffice")[] {
  if (!diag?.engines) return [];
  return (["ffmpeg", "libreoffice"] as const).filter((e) => !diag.engines!.includes(e));
}

/** The facts for a bug report, as plain text. Versions, system and test
 * results only: never file names, contents or paths (the startup error is
 * what Settings already shows). */
export function diagnosticsText(d: Diagnostics): string {
  const failed = (d.sandbox?.attempts ?? []).filter((a) => !a.passed);
  const lines = [
    "Local File Conversion Suite diagnostics",
    `Version: ${d.appVersion ?? "unknown"}`,
    `System: ${d.platform ?? "unknown"}`,
    `Worker protocol: ${d.protocolVersion ?? "unknown"}`,
    `Processing: ${d.processingEnabled ? "enabled" : "disabled"}`,
    `Engines found: ${(d.engines ?? []).join(", ") || "none"}`,
    `Tools: ${(d.adapters ?? []).join(", ") || "none"}`,
    d.sandbox
      ? `Sandbox self-test: ${d.sandbox.passed ? "passed" : "failed"} (${d.sandbox.attempts.length - failed.length} of ${d.sandbox.attempts.length} checks passed)`
      : "Sandbox self-test: not run yet",
    ...failed.map((a) => `  failed: ${a.name} (expected ${a.expected}, got ${a.outcome})`),
  ];
  if (d.startupError) lines.push(`Startup error: ${d.startupError}`);
  return lines.join("\n");
}

/** Puts text on the clipboard through the browser API. It needs no Tauri
 * permission, and the CSP doesn't restrict it. False when the clipboard is
 * missing or refuses, so the screen can show the text to copy by hand. */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (typeof navigator === "undefined" || !navigator.clipboard?.writeText) return false;
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
