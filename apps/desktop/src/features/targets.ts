import type { Api, Targets } from "../api";
import { actionError } from "./actionError";

const NONE: Targets = { operations: [], groups: [], shared: [] };

/** What the selected files can become. A failure comes back as words for the
 * screen, with an empty list so the rest of the screen still works; it used
 * to be swallowed, so "nothing to convert to" could mean anything (audit U28). */
export async function loadTargets(api: Api, ids: string[]): Promise<{ targets: Targets; error: string | null }> {
  try {
    return { targets: await api.getTargets(ids), error: null };
  } catch (e) {
    return { targets: NONE, error: actionError(e) };
  }
}
