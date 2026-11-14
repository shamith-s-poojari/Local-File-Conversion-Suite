import { useState } from "react";
import type { WidgetProps } from "./types";

/** `password` widget: masked, with a reveal toggle. The value lives only in
 * form state and is dropped from machine context once submitted. */
export function Password({ id, label, schema, value, onChange }: WidgetProps<string>) {
  const [shown, setShown] = useState(false);
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <div className="row">
        <input
          id={id}
          type={shown ? "text" : "password"}
          autoComplete="new-password"
          spellCheck={false}
          value={value ?? ""}
          onChange={(e) => onChange(e.target.value || undefined)}
        />
        <button type="button" onClick={() => setShown((s) => !s)} aria-pressed={shown}>
          {shown ? "Hide" : "Show"}
        </button>
      </div>
      {schema.description && <p className="hint">{schema.description}</p>}
    </div>
  );
}
