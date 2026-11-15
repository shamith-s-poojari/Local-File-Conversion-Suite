// The `pageRanges` syntax, e.g. `1-3, 5, 8-`. Matches
// `converter_sdk.pdf.parse_page_ranges`, so the UI and `validateInput`
// accept exactly the same input (spec 12.3).

export type RangeReason = "empty" | "syntax" | "outOfBounds" | "reversed" | "tooLong";

export type Parsed = { ok: true; ranges: [number, number][] } | { ok: false; reason: RangeReason };

const PART = /^\s*(\d*)\s*(-?)\s*(\d*)\s*$/;

export function parsePageRanges(spec: string, total: number): Parsed {
  if (spec.length > 10_000) return { ok: false, reason: "tooLong" };
  const parts = spec.split(",").filter((p) => p.trim() !== "");
  if (parts.length === 0) return { ok: false, reason: "empty" };
  const ranges: [number, number][] = [];
  for (const part of parts) {
    const m = PART.exec(part);
    if (!m || (!m[1] && !m[3])) return { ok: false, reason: "syntax" };
    const first = m[1] ? Number(m[1]) : 1;
    const last = m[2] ? (m[3] ? Number(m[3]) : total) : first;
    if (first < 1 || last > total) return { ok: false, reason: "outOfBounds" };
    if (first > last) return { ok: false, reason: "reversed" };
    ranges.push([first, last]);
  }
  return { ok: true, ranges };
}

export const RANGE_TEXT: Record<RangeReason, string> = {
  empty: "Enter at least one page or range.",
  syntax: "Use page numbers and ranges like 1-3, 5, 8-",
  outOfBounds: "A page number is outside this document.",
  reversed: "A range runs backwards, like 5-3.",
  tooLong: "That's too long.",
};
