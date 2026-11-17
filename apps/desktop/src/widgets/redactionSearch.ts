import type { PageText } from "../api";

export const SEARCH_PAGE_LIMIT = 500;

export interface Match {
  page: number;
  count: number;
}

/** Counts, per page, how often the terms appear in the text layer. Words are
 * joined without spaces and compared in lower case, so a term still matches
 * across a line break. Returns null if `signal` was aborted (the partial
 * list would mislead); a page that can't be read ends the search with what
 * was found so far. */
export async function findMatches({
  total,
  getText,
  needles,
  signal,
  limit = SEARCH_PAGE_LIMIT,
}: {
  total: number;
  getText: (page: number) => Promise<PageText>;
  needles: string[];
  signal: AbortSignal;
  limit?: number;
}): Promise<Match[] | null> {
  const wanted = needles.map((t) => t.toLowerCase().replace(/\s+/g, "")).filter(Boolean);
  const found: Match[] = [];
  for (let page = 1; page <= Math.min(total, limit); page++) {
    if (signal.aborted) return null;
    let text: PageText;
    try {
      text = await getText(page);
    } catch {
      break;
    }
    if (signal.aborted) return null;
    const joined = text.words.map((w) => w.text).join("").toLowerCase();
    const count = wanted.reduce((n, needle) => n + joined.split(needle).length - 1, 0);
    if (count) found.push({ page, count });
  }
  return found;
}
