import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";
import type { Api, WordBox } from "../api";
import { noApi } from "../fixtures";
import { REDACTION_KEYBOARD_HINT } from "../text";
import { PageOrganizer } from "./PageOrganizer";
import { duplicateItem, moveItem, removeItem, rotateItem, type Item } from "./organizerOps";
import { RedactionCanvas } from "./RedactionCanvas";
import { findMatches } from "./redactionSearch";

const items = (...sources: number[]): Item[] => sources.map((s, i) => ({ id: `i${i}`, entry: { source: s } }));
const ids = (list: Item[]) => list.map((i) => i.id);

describe("page organizer operations keep each page's identity (U14)", () => {
  test("moving a page moves its id with it", () => {
    const moved = moveItem(items(1, 2, 3), 0, 1);
    expect(ids(moved)).toEqual(["i1", "i0", "i2"]);
    expect(moved.map((i) => i.entry.source)).toEqual([2, 1, 3]);
  });
  test("a duplicate gets a new id and the original keeps its own", () => {
    const d = duplicateItem(items(1, 2), 0, "new");
    expect(ids(d)).toEqual(["i0", "new", "i1"]);
    expect(d[1]!.entry).toEqual(d[0]!.entry);
  });
  test("deleting keeps the others' ids, and the last page can't be deleted", () => {
    expect(ids(removeItem(items(1, 2, 3), 1))).toEqual(["i0", "i2"]);
    expect(ids(removeItem(items(1), 0))).toEqual(["i0"]);
  });
  test("rotating turns clockwise and wraps", () => {
    let l = items(1);
    const turns = [];
    for (let k = 0; k < 4; k++) {
      l = rotateItem(l, 0);
      turns.push(l[0]!.entry.rotate);
    }
    expect(turns).toEqual([90, 180, 270, 0]);
    expect(l[0]!.id).toBe("i0");
  });
  test("moving past the ends changes nothing", () => {
    expect(ids(moveItem(items(1, 2), 0, -1))).toEqual(["i0", "i1"]);
    expect(ids(moveItem(items(1, 2), 1, 1))).toEqual(["i0", "i1"]);
  });
});

describe("PageOrganizer", () => {
  const ctx = { files: [], pages: 3, reference: "f", api: noApi };
  const render = (value?: { source: number }[]) =>
    renderToStaticMarkup(
      <PageOrganizer id="p" label="Pages" schema={{}} value={value} onChange={() => {}} ctx={ctx} params={{}} setParam={() => {}} />,
    );
  test("has a Reset button and a live announcement area", () => {
    const m = render([{ source: 2 }, { source: 1 }, { source: 3 }]);
    expect(m).toContain("Reset");
    expect(m).toMatch(/role="status"/);
  });
  test("every page can be found by its id, so focus can follow it", () => {
    const m = render();
    expect((m.match(/data-page-id="/g) ?? []).length).toBe(3);
  });
  test("Reset waits until something has changed", () => {
    expect(render()).toMatch(/<button[^>]*disabled=""[^>]*>Reset<\/button>/);
    expect(render([{ source: 2 }, { source: 1 }, { source: 3 }])).not.toMatch(/<button[^>]*disabled=""[^>]*>Reset<\/button>/);
  });
});

describe("redaction search (U17)", () => {
  const words = (...texts: string[]): { words: WordBox[] } => ({
    words: texts.map((text, i) => ({ text, x0: i, y0: 0, x1: i + 1, y1: 1 })),
  });
  const pages = [words("Jane", "Doe"), words("nothing"), words("jane", "doe", "again", "Jane", "Doe")];
  const getText = async (p: number) => ({ width: 1, height: 1, ...pages[p - 1]! });

  test("counts matches per page, ignoring case and spaces", async () => {
    const r = await findMatches({ total: 3, getText, needles: ["jane doe"], signal: new AbortController().signal });
    expect(r).toEqual([
      { page: 1, count: 1 },
      { page: 3, count: 2 },
    ]);
  });

  test("stops asking for pages once cancelled", async () => {
    const ctl = new AbortController();
    const asked: number[] = [];
    const r = await findMatches({
      total: 3,
      needles: ["jane"],
      signal: ctl.signal,
      getText: async (p) => {
        asked.push(p);
        if (p === 1) ctl.abort();
        return getText(p);
      },
    });
    expect(asked).toEqual([1]);
    expect(r).toBeNull();
  });

  test("a page that can't be read ends the search with what was found", async () => {
    const r = await findMatches({
      total: 3,
      needles: ["jane"],
      signal: new AbortController().signal,
      getText: async (p) => {
        if (p === 2) throw new Error("x");
        return getText(p);
      },
    });
    expect(r).toEqual([{ page: 1, count: 1 }]);
  });

  test("only the first pages are searched", async () => {
    const asked: number[] = [];
    await findMatches({
      total: 1000,
      limit: 5,
      needles: ["x"],
      signal: new AbortController().signal,
      getText: async (p) => (asked.push(p), { width: 1, height: 1, words: [] }),
    });
    expect(asked).toEqual([1, 2, 3, 4, 5]);
  });
});

describe("RedactionCanvas", () => {
  test("says how to do it from the keyboard (U17)", () => {
    const m = renderToStaticMarkup(
      <RedactionCanvas
        id="r"
        label="Areas"
        schema={{}}
        value={undefined}
        onChange={() => {}}
        ctx={{ files: [], pages: 2, reference: "f", api: noApi as Api }}
        params={{}}
        setParam={() => {}}
      />,
    );
    expect(m).toContain(REDACTION_KEYBOARD_HINT.replace(/'/g, "&#x27;"));
    expect(m).toContain("Redact this whole page");
  });
});
