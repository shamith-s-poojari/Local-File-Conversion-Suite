/** An `aria-describedby` value: the IDs that are set, space separated, or
 * undefined when none is (so no empty attribute is written). */
export function describedBy(...ids: (string | false | null | undefined)[]): string | undefined {
  const set = ids.filter((i): i is string => typeof i === "string" && i !== "");
  return set.length > 0 ? set.join(" ") : undefined;
}

/** The text of a textarea as the list it stands for: one item per line,
 * blank lines left out. */
export function parseLines(text: string): string[] | undefined {
  const items = text.split("\n").filter((x) => x.trim() !== "");
  return items.length > 0 ? items : undefined;
}

/** Whether the typed text already stands for `list`: then the box keeps
 * what the person typed (a trailing newline, a blank line between items). */
export function sameLines(text: string, list: string[] | undefined): boolean {
  const parsed = parseLines(text) ?? [];
  const want = list ?? [];
  return parsed.length === want.length && parsed.every((x, i) => x === want[i]);
}
