import { reasonMessage, type Message } from "../text";

/** The words for an error from a command: its `reason` when the app has
 * text for it, otherwise the text for its `code`. */
export function actionMessage(e: unknown): Message {
  const err = (typeof e === "object" && e !== null ? e : {}) as { code?: string; reason?: string };
  return reasonMessage(err.code ?? "E_INTERNAL", err.reason);
}

export function actionError(e: unknown): string {
  return actionMessage(e).what;
}
