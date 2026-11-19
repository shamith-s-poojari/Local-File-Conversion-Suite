import type { WordBox } from "../api";

/** Normalized 0..1 in displayed orientation, origin top-left (spec 13.8). */
export interface Box {
  page: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** A line of text on a page, in the page's own units. */
export interface Line {
  text: string;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** The most lines listed at once; a page has rarely more. */
export const MAX_LINES = 300;

/** A gap wider than this many line heights starts a new line: a column or a table cell. */
const COLUMN_GAP = 2.5;

/**
 * Groups a page's words into lines for a list a person can walk with the
 * keyboard. Words on the same row (their heights overlap by half) are one
 * line, read left to right; a wide gap splits columns. Lines come top to
 * bottom. The backend gives words only, so lines are worked out here.
 */
export function groupLines(words: WordBox[]): Line[] {
  const usable = words
    .filter((w) => w.text.trim() !== "" && [w.x0, w.y0, w.x1, w.y1].every(Number.isFinite) && w.y1 > w.y0)
    .sort((a, b) => a.y0 + a.y1 - (b.y0 + b.y1));
  const rows: WordBox[][] = [];
  let top = 0;
  let bottom = 0;
  for (const w of usable) {
    const last = rows[rows.length - 1];
    const overlap = Math.min(bottom, w.y1) - Math.max(top, w.y0);
    if (last && overlap >= 0.5 * Math.min(bottom - top, w.y1 - w.y0)) {
      last.push(w);
      top = Math.min(top, w.y0);
      bottom = Math.max(bottom, w.y1);
    } else {
      rows.push([w]);
      top = w.y0;
      bottom = w.y1;
    }
  }
  const lines: Line[] = [];
  for (const row of rows) {
    row.sort((a, b) => a.x0 - b.x0);
    let part: WordBox[] = [];
    const flush = () => {
      if (part.length === 0) return;
      lines.push({
        text: part.map((w) => w.text.trim()).join(" "),
        x0: Math.min(...part.map((w) => w.x0)),
        y0: Math.min(...part.map((w) => w.y0)),
        x1: Math.max(...part.map((w) => w.x1)),
        y1: Math.max(...part.map((w) => w.y1)),
      });
      part = [];
    };
    for (const w of row) {
      const prev = part[part.length - 1];
      if (prev && w.x0 - prev.x1 > COLUMN_GAP * (w.y1 - w.y0)) flush();
      part.push(w);
    }
    flush();
  }
  return lines.sort((a, b) => a.y0 - b.y0 || a.x0 - b.x0);
}

/** A little room around what's covered, so no edge of a letter shows. */
export function pad(b: Box): Box {
  const d = 0.002;
  return { page: b.page, x0: Math.max(0, b.x0 - d), y0: Math.max(0, b.y0 - d), x1: Math.min(1, b.x1 + d), y1: Math.min(1, b.y1 + d) };
}

/** The box that covers `line`, on a page of `width` x `height` units. */
export function lineBox(line: Line, page: number, width: number, height: number): Box {
  return pad({ page, x0: line.x0 / width, y0: line.y0 / height, x1: line.x1 / width, y1: line.y1 / height });
}

/** Whether a box already covers all of `target` (on the same page). */
export function isCovered(boxes: Box[], target: Box): boolean {
  const e = 1e-6;
  return boxes.some((b) => b.page === target.page && b.x0 <= target.x0 + e && b.y0 <= target.y0 + e && b.x1 >= target.x1 - e && b.y1 >= target.y1 - e);
}
