import { useEffect, useState } from "react";
import type { Api } from "../api";
import type { Event } from "../machines/app";
import { actionError } from "./actionError";

/** Tab-separated lines, as Excel, Sheets and web tables put on the clipboard. */
export function looksTabular(text: string): boolean {
  const lines = text.split(/\r?\n/).filter((l) => l.trim() !== "");
  return lines.length >= 2 && lines.filter((l) => l.includes("\t")).length >= lines.length * 0.8;
}

/** What a paste becomes: a picture, a table or text (ADR 0005). */
export async function readPaste(
  data: DataTransfer,
): Promise<{ kind: "image" | "table" | "text"; bytes: Uint8Array } | null> {
  const image = Array.from(data.files).find((f) => f.type === "image/png" || f.type === "image/jpeg");
  if (image) return { kind: "image", bytes: new Uint8Array(await image.arrayBuffer()) };
  const text = data.getData("text/plain");
  if (!text.trim()) return null;
  return { kind: looksTabular(text) ? "table" : "text", bytes: new TextEncoder().encode(text) };
}

/** Takes pastes while `enabled`, except into a text field, which keeps its
 * own. Returns the last problem, if any, and a way to clear it. */
export function usePaste(api: Api, enabled: boolean, send: (e: Event) => void) {
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const onPaste = (e: ClipboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest?.("input, textarea, [contenteditable]")) return;
      if (!e.clipboardData) return;
      e.preventDefault();
      setProblem(null);
      void readPaste(e.clipboardData)
        .then((p) => (p ? api.addPasted(p.kind, p.bytes) : []))
        .then((results) => results.length && send({ type: "FILES_ADDED", results }))
        .catch((err) => setProblem(actionError(err)));
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [enabled, api, send]);
  return { problem, dismiss: () => setProblem(null) };
}
