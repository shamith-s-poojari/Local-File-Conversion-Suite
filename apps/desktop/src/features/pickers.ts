import { useState } from "react";
import type { Api } from "../api";
import type { Event } from "../machines/app";
import { actionError } from "./actionError";

/** Opens the native file or folder dialog and sends what was chosen. Returns
 * words for the screen when the dialog fails, otherwise null: a failure used
 * to leave the button doing nothing (audit U6). */
export async function runPick(api: Api, kind: "files" | "folder", send: (e: Event) => void): Promise<string | null> {
  try {
    const results = kind === "folder" ? await api.pickFolder() : await api.pickFiles();
    if (results.length) send({ type: "FILES_ADDED", results });
    return null;
  } catch (e) {
    return actionError(e);
  }
}

/** The pickers' busy flag and last problem, for the screen that has the buttons. */
export function usePicker(api: Api, send: (e: Event) => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const choose = async (kind: "files" | "folder") => {
    setBusy(true);
    setError(null);
    setError(await runPick(api, kind, send));
    setBusy(false);
  };
  return { busy, error, choose };
}
