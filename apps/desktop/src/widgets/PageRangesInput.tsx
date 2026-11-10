import { parsePageRanges, RANGE_TEXT } from "./pageRanges";
import type { WidgetProps } from "./types";

/** `pageRanges` widget: `1-3, 5, 8-`, validated live against the page count. */
export function PageRangesInput({ id, label, schema, value, onChange, ctx }: WidgetProps<string>) {
  const text = value ?? "";
  const parsed = text.trim() && ctx.pages > 0 ? parsePageRanges(text, ctx.pages) : null;
  const error = parsed && !parsed.ok ? RANGE_TEXT[parsed.reason] : null;
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        value={text}
        placeholder={ctx.pages ? `1-${ctx.pages}` : "1-3, 5, 8-"}
        aria-invalid={error ? true : undefined}
        aria-describedby={`${id}-help`}
        onChange={(e) => onChange(e.target.value || undefined)}
      />
      <p id={`${id}-help`} className={error ? "hint error" : "hint"} role={error ? "alert" : undefined}>
        {error ?? schema.description ?? (ctx.pages ? `This document has ${ctx.pages} pages.` : "")}
      </p>
    </div>
  );
}
