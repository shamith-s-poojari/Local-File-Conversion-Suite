import type { WidgetProps } from "./types";

/** `fileOrder` widget: reorder files with buttons, so it works by keyboard
 * as well as pointer (spec 12.4). The value is the list of file IDs. */
export function FileOrder({ id, label, value, onChange, ctx }: WidgetProps<string[]>) {
  const all = ctx.files.flatMap((f) => (f.file?.actionable ? [f.file] : []));
  const order = value && value.length === all.length ? value : all.map((f) => f.fileId);
  const name = (fid: string) => all.find((f) => f.fileId === fid)?.displayName ?? fid;
  const move = (i: number, by: number) => {
    const next = [...order];
    const [item] = next.splice(i, 1);
    next.splice(i + by, 0, item!);
    onChange(next);
  };
  return (
    <fieldset className="field" id={id}>
      <legend>{label}</legend>
      <ol className="file-order">
        {order.map((fid, i) => (
          <li key={fid}>
            <span className="name">{name(fid)}</span>
            <button type="button" disabled={i === 0} onClick={() => move(i, -1)} aria-label={`Move ${name(fid)} up`}>
              ↑
            </button>
            <button
              type="button"
              disabled={i === order.length - 1}
              onClick={() => move(i, 1)}
              aria-label={`Move ${name(fid)} down`}
            >
              ↓
            </button>
          </li>
        ))}
      </ol>
    </fieldset>
  );
}
