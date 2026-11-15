import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import { previewUrl, type WordBox } from "../api";
import type { WidgetProps } from "./types";

/** Normalized 0..1 in displayed orientation, origin top-left (spec 13.8). */
export interface Box {
  page: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

const DPI = 96;
/** A drag shorter than this (normalized) is a click: snap to the word under it. */
const CLICK = 0.004;
const SEARCH_PAGE_LIMIT = 500;

/** `redactionCanvas` widget (spec 12.3): draw, select and remove boxes;
 * word snapping; term search with a results list; undo. Everything except
 * drawing is also reachable by keyboard: whole-page redaction and terms. */
export function RedactionCanvas({ id, label, value, onChange, ctx, params, setParam }: WidgetProps<Box[]>) {
  const boxes = value ?? [];
  const terms = (params.terms as string[] | undefined) ?? [];
  const [page, setPage] = useState(1);
  const [img, setImg] = useState<{ src: string; w: number; h: number } | null>(null);
  const [words, setWords] = useState<{ w: number; h: number; words: WordBox[] } | null>(null);
  const [drag, setDrag] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const [history, setHistory] = useState<Box[][]>([]);
  const [term, setTerm] = useState("");
  const [results, setResults] = useState<{ page: number; count: number }[] | null>(null);
  const [searching, setSearching] = useState(false);
  const surface = useRef<HTMLDivElement>(null);
  const total = Math.max(ctx.pages, 1);

  useEffect(() => {
    let live = true;
    setImg(null);
    ctx.api
      .requestPreview(ctx.reference, page, DPI)
      .then((p) => live && setImg({ src: previewUrl(p.path), w: p.widthPx, h: p.heightPx }))
      .catch(() => live && setImg(null));
    ctx.api
      .getPageText(ctx.reference, page)
      .then((t) => live && setWords({ w: t.width, h: t.height, words: t.words }))
      .catch(() => live && setWords(null));
    return () => {
      live = false;
    };
  }, [ctx.api, ctx.reference, page]);

  const commit = useCallback(
    (next: Box[]) => {
      setHistory((h) => [...h.slice(-49), boxes]);
      onChange(next);
    },
    [boxes, onChange],
  );

  const point = (e: PointerEvent) => {
    const r = surface.current!.getBoundingClientRect();
    return {
      x: Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)),
      y: Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)),
    };
  };

  const onDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    (e.target as Element).setPointerCapture?.(e.pointerId);
    const p = point(e);
    setDrag({ x0: p.x, y0: p.y, x1: p.x, y1: p.y });
  };
  const onMove = (e: PointerEvent) => {
    if (!drag) return;
    const p = point(e);
    setDrag({ ...drag, x1: p.x, y1: p.y });
  };
  const onUp = () => {
    if (!drag) return;
    const d = drag;
    setDrag(null);
    if (Math.abs(d.x1 - d.x0) < CLICK && Math.abs(d.y1 - d.y0) < CLICK) {
      // Word snapping: a click covers the word under the pointer.
      const w = words?.words.find(
        (w) => d.x0 >= w.x0 / words.w && d.x0 <= w.x1 / words.w && d.y0 >= w.y0 / words.h && d.y0 <= w.y1 / words.h,
      );
      if (w && words) {
        commit([...boxes, pad({ page, x0: w.x0 / words.w, y0: w.y0 / words.h, x1: w.x1 / words.w, y1: w.y1 / words.h })]);
      }
      return;
    }
    commit([
      ...boxes,
      { page, x0: Math.min(d.x0, d.x1), y0: Math.min(d.y0, d.y1), x1: Math.max(d.x0, d.x1), y1: Math.max(d.y0, d.y1) },
    ]);
  };

  const undo = () => {
    const prev = history[history.length - 1];
    if (!prev) return;
    setHistory((h) => h.slice(0, -1));
    onChange(prev);
  };

  const addTerm = () => {
    const t = term.trim();
    if (t.length < 2 || terms.includes(t)) return;
    setParam("terms", [...terms, t]);
    setTerm("");
    setResults(null);
  };

  const search = async () => {
    if (terms.length === 0) return;
    setSearching(true);
    const found: { page: number; count: number }[] = [];
    const needles = terms.map((t) => t.toLowerCase().replace(/\s+/g, ""));
    for (let p = 1; p <= Math.min(total, SEARCH_PAGE_LIMIT); p++) {
      try {
        const t = await ctx.api.getPageText(ctx.reference, p);
        const text = t.words.map((w) => w.text).join("").toLowerCase();
        const count = needles.reduce((n, needle) => n + text.split(needle).length - 1, 0);
        if (count) found.push({ page: p, count });
      } catch {
        break;
      }
    }
    setResults(found);
    setSearching(false);
  };

  const highlights =
    words && terms.length
      ? words.words.filter((w) => terms.some((t) => w.text.toLowerCase().includes(t.toLowerCase().split(/\s+/)[0]!)))
      : [];
  const onPage = boxes.map((b, i) => ({ b, i })).filter(({ b }) => b.page === page);
  const ratio = img ? `${img.w} / ${img.h}` : words ? `${words.w} / ${words.h}` : "612 / 792";

  return (
    <fieldset className="field redaction" id={id}>
      <legend>{label}</legend>
      <div className="redaction-layout">
        <div>
          <div className="row pager">
            <button type="button" disabled={page <= 1} onClick={() => setPage(page - 1)}>
              Previous page
            </button>
            <span aria-live="polite">
              Page {page} of {total}
            </span>
            <button type="button" disabled={page >= total} onClick={() => setPage(page + 1)}>
              Next page
            </button>
          </div>
          <div
            ref={surface}
            className="redaction-surface"
            style={{ aspectRatio: ratio }}
            onPointerDown={onDown}
            onPointerMove={onMove}
            onPointerUp={onUp}
            role="img"
            aria-label={`Page ${page}. Drag to cover an area, or click a word.`}
          >
            {img ? <img src={img.src} alt="" draggable={false} /> : <div className="loading">Rendering page…</div>}
            <svg viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true">
              {words &&
                highlights.map((w, i) => (
                  <rect
                    key={`h${i}`}
                    className="match"
                    x={w.x0 / words.w}
                    y={w.y0 / words.h}
                    width={(w.x1 - w.x0) / words.w}
                    height={(w.y1 - w.y0) / words.h}
                  />
                ))}
              {onPage.map(({ b, i }) => (
                <rect key={i} className="box" x={b.x0} y={b.y0} width={b.x1 - b.x0} height={b.y1 - b.y0} />
              ))}
              {drag && (
                <rect
                  className="box drawing"
                  x={Math.min(drag.x0, drag.x1)}
                  y={Math.min(drag.y0, drag.y1)}
                  width={Math.abs(drag.x1 - drag.x0)}
                  height={Math.abs(drag.y1 - drag.y0)}
                />
              )}
            </svg>
          </div>
        </div>
        <div className="redaction-side">
          <div className="row">
            <button type="button" onClick={() => commit([...boxes, { page, x0: 0, y0: 0, x1: 1, y1: 1 }])}>
              Redact this whole page
            </button>
            <button type="button" onClick={undo} disabled={history.length === 0}>
              Undo
            </button>
          </div>
          <h4>Areas ({boxes.length})</h4>
          {boxes.length === 0 ? (
            <p className="hint">Drag on the page to cover an area, or click a word.</p>
          ) : (
            <ul className="box-list">
              {boxes.map((b, i) => (
                <li key={i}>
                  <button type="button" className="link" onClick={() => setPage(b.page)}>
                    Page {b.page}
                    {b.x0 === 0 && b.y0 === 0 && b.x1 === 1 && b.y1 === 1 ? " (whole page)" : ""}
                  </button>
                  <button type="button" onClick={() => commit(boxes.filter((_, j) => j !== i))} aria-label={`Remove area ${i + 1}`}>
                    Remove
                  </button>
                </li>
              ))}
            </ul>
          )}
          <h4>Search terms</h4>
          <div className="row">
            <label htmlFor={`${id}-term`} className="visually-hidden">
              Term to redact everywhere
            </label>
            <input
              id={`${id}-term`}
              value={term}
              placeholder="Text to remove everywhere"
              onChange={(e) => setTerm(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addTerm();
                }
              }}
            />
            <button type="button" onClick={addTerm} disabled={term.trim().length < 2}>
              Add
            </button>
          </div>
          <ul className="term-list">
            {terms.map((t) => (
              <li key={t}>
                <span className="name">{t}</span>
                <button type="button" onClick={() => setParam("terms", terms.filter((x) => x !== t))} aria-label={`Remove term ${t}`}>
                  Remove
                </button>
              </li>
            ))}
          </ul>
          {terms.length > 0 && (
            <button type="button" onClick={search} disabled={searching}>
              {searching ? "Searching…" : "Find matches"}
            </button>
          )}
          {results && (
            <div aria-live="polite">
              {results.length === 0 ? (
                <p className="hint">No matches in the text layer. Scanned pages can't be searched.</p>
              ) : (
                <ul className="results">
                  {results.map((r) => (
                    <li key={r.page}>
                      <button type="button" className="link" onClick={() => setPage(r.page)}>
                        Page {r.page}: {r.count} match{r.count === 1 ? "" : "es"}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              {total > SEARCH_PAGE_LIMIT && (
                <p className="hint">Only the first {SEARCH_PAGE_LIMIT} pages were listed; terms apply to every page.</p>
              )}
            </div>
          )}
          <p className="hint">Every term is removed from the whole document, whatever the list shows.</p>
        </div>
      </div>
    </fieldset>
  );
}

function pad(b: Box): Box {
  const d = 0.002;
  return { page: b.page, x0: Math.max(0, b.x0 - d), y0: Math.max(0, b.y0 - d), x1: Math.min(1, b.x1 + d), y1: Math.min(1, b.y1 + d) };
}
