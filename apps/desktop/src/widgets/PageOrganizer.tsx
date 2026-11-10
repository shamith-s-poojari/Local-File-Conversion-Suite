import type { WidgetProps } from "./types";
import { Thumbnail } from "./Thumbnail";

interface Entry {
  source: number;
  rotate?: 0 | 90 | 180 | 270;
}

/** `pageOrganizer` widget: reorder, rotate, delete and duplicate pages, all
 * with buttons (keyboard-operable). Thumbnails load lazily. */
export function PageOrganizer({ id, label, value, onChange, ctx }: WidgetProps<Entry[]>) {
  const pages: Entry[] =
    value && value.length > 0 ? value : Array.from({ length: ctx.pages }, (_, i) => ({ source: i + 1 }));
  const set = (next: Entry[]) => onChange(next);
  const move = (i: number, by: number) => {
    const next = [...pages];
    const [item] = next.splice(i, 1);
    next.splice(i + by, 0, item!);
    set(next);
  };
  const rotate = (i: number) =>
    set(pages.map((p, j) => (j === i ? { ...p, rotate: ((((p.rotate ?? 0) + 90) % 360) as Entry["rotate"]) } : p)));
  const remove = (i: number) => pages.length > 1 && set(pages.filter((_, j) => j !== i));
  const duplicate = (i: number) => set([...pages.slice(0, i + 1), { ...pages[i]! }, ...pages.slice(i + 1)]);

  return (
    <fieldset className="field" id={id}>
      <legend>{label}</legend>
      <p className="hint">
        {pages.length} page{pages.length === 1 ? "" : "s"} in the result. Pages you delete are left out.
      </p>
      <ol className="organizer">
        {pages.map((p, i) => (
          <li key={`${i}-${p.source}`}>
            <Thumbnail api={ctx.api} reference={ctx.reference} page={p.source} rotate={p.rotate ?? 0} alt={`Page ${p.source}`} />
            <span className="caption">
              Page {p.source}
              {p.rotate ? ` · ${p.rotate}°` : ""}
            </span>
            <div className="tools">
              <button type="button" disabled={i === 0} onClick={() => move(i, -1)} aria-label={`Move page ${p.source} earlier`}>
                ←
              </button>
              <button type="button" disabled={i === pages.length - 1} onClick={() => move(i, 1)} aria-label={`Move page ${p.source} later`}>
                →
              </button>
              <button type="button" onClick={() => rotate(i)} aria-label={`Rotate page ${p.source} clockwise`}>
                ↻
              </button>
              <button type="button" onClick={() => duplicate(i)} aria-label={`Duplicate page ${p.source}`}>
                ⧉
              </button>
              <button type="button" disabled={pages.length === 1} onClick={() => remove(i)} aria-label={`Delete page ${p.source}`}>
                ✕
              </button>
            </div>
          </li>
        ))}
      </ol>
    </fieldset>
  );
}
