import { useEffect, useRef, useState } from "react";
import { PAGE_ORGANIZER } from "../text";
import { duplicateItem, moveItem, removeItem, rotateItem, type Entry, type Item } from "./organizerOps";
import { Thumbnail } from "./Thumbnail";
import type { WidgetProps } from "./types";

type Action = "earlier" | "later" | "rotate" | "duplicate" | "delete";

/** `pageOrganizer` widget: reorder, rotate, delete and duplicate pages, all
 * with buttons (keyboard-operable). Thumbnails load lazily. After each change
 * focus stays on the page that moved and the change is announced. */
export function PageOrganizer({ id, label, value, onChange, ctx, invalid, errorId }: WidgetProps<Entry[]>) {
  const entries: Entry[] =
    value && value.length > 0 ? value : Array.from({ length: ctx.pages }, (_, i) => ({ source: i + 1 }));
  // Stable ids, one per page, kept beside the value.
  const ids = useRef<string[]>([]);
  const counter = useRef(0);
  const fresh = () => `pg${++counter.current}`;
  if (ids.current.length !== entries.length) ids.current = entries.map(fresh);
  const items: Item[] = entries.map((entry, i) => ({ id: ids.current[i]!, entry }));

  const [status, setStatus] = useState("");
  const list = useRef<HTMLOListElement>(null);
  const focusAfter = useRef<{ id: string; action: Action } | null>(null);

  useEffect(() => {
    const want = focusAfter.current;
    if (!want) return;
    focusAfter.current = null;
    const row = list.current?.querySelector<HTMLElement>(`[data-page-id="${want.id}"]`);
    const buttons = Array.from(row?.querySelectorAll<HTMLButtonElement>("button[data-action]") ?? []);
    // The button it was on, or - at the end of the list - the other way.
    const target =
      buttons.find((b) => b.dataset.action === want.action && !b.disabled) ??
      buttons.find((b) => (b.dataset.action === "earlier" || b.dataset.action === "later") && !b.disabled) ??
      buttons.find((b) => !b.disabled);
    target?.focus();
  });

  const commit = (next: Item[], message: string, focus: { id: string; action: Action }) => {
    ids.current = next.map((n) => n.id);
    focusAfter.current = focus;
    setStatus(message);
    onChange(next.map((n) => n.entry));
  };
  const move = (i: number, by: number) => {
    const it = items[i]!;
    commit(moveItem(items, i, by), PAGE_ORGANIZER.moved(it.entry.source, i + by + 1, items.length), {
      id: it.id,
      action: by < 0 ? "earlier" : "later",
    });
  };
  const rotate = (i: number) => {
    const next = rotateItem(items, i);
    commit(next, PAGE_ORGANIZER.rotated(next[i]!.entry.source, next[i]!.entry.rotate ?? 0), { id: next[i]!.id, action: "rotate" });
  };
  const duplicate = (i: number) => {
    const next = duplicateItem(items, i, fresh());
    commit(next, PAGE_ORGANIZER.duplicated(items[i]!.entry.source, next.length), { id: items[i]!.id, action: "duplicate" });
  };
  const remove = (i: number) => {
    if (items.length < 2) return;
    const next = removeItem(items, i);
    const neighbour = next[Math.min(i, next.length - 1)]!;
    commit(next, PAGE_ORGANIZER.deleted(items[i]!.entry.source, next.length), { id: neighbour.id, action: "delete" });
  };
  const reset = () => {
    ids.current = [];
    focusAfter.current = null;
    setStatus(PAGE_ORGANIZER.reset(ctx.pages));
    onChange(undefined);
  };

  return (
    <fieldset className="field" id={id} aria-describedby={invalid ? errorId : undefined}>
      <legend>{label}</legend>
      <div className="row">
        <p className="hint">
          {items.length} page{items.length === 1 ? "" : "s"} in the result. Pages you delete are left out.
        </p>
        <button type="button" className="ghost sm" onClick={reset} disabled={!value}>
          Reset
        </button>
      </div>
      <p className="visually-hidden" role="status">
        {status}
      </p>
      <ol className="organizer" ref={list}>
        {items.map(({ id: pageId, entry: p }, i) => (
          <li key={pageId} data-page-id={pageId}>
            <Thumbnail api={ctx.api} reference={ctx.reference} page={p.source} rotate={p.rotate ?? 0} alt={`Page ${p.source}`} />
            <span className="caption">
              Page {p.source}
              {p.rotate ? ` · ${p.rotate}°` : ""}
            </span>
            <div className="tools">
              <button type="button" data-action="earlier" disabled={i === 0} onClick={() => move(i, -1)} aria-label={`Move page ${p.source} earlier`}>
                ←
              </button>
              <button
                type="button"
                data-action="later"
                disabled={i === items.length - 1}
                onClick={() => move(i, 1)}
                aria-label={`Move page ${p.source} later`}
              >
                →
              </button>
              <button type="button" data-action="rotate" onClick={() => rotate(i)} aria-label={`Rotate page ${p.source} clockwise`}>
                ↻
              </button>
              <button type="button" data-action="duplicate" onClick={() => duplicate(i)} aria-label={`Duplicate page ${p.source}`}>
                ⧉
              </button>
              <button type="button" data-action="delete" disabled={items.length === 1} onClick={() => remove(i)} aria-label={`Delete page ${p.source}`}>
                ✕
              </button>
            </div>
          </li>
        ))}
      </ol>
    </fieldset>
  );
}
