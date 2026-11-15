import type { WidgetProps } from "./types";

const HEX = /^#[0-9A-Fa-f]{6}$/;

/** A colour as `#RRGGBB`, with the system picker and a text field (`x-ui: color`). */
export function ColorInput({ id, label, schema, value, onChange }: WidgetProps<string>) {
  const current = typeof value === "string" ? value : typeof schema.default === "string" ? schema.default : "#000000";
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <div className="color-row">
        <input
          id={id}
          type="color"
          className="swatch"
          value={HEX.test(current) ? current : "#000000"}
          onChange={(e) => onChange(e.target.value.toUpperCase())}
        />
        <input
          aria-label={`${label} (hex)`}
          className="mono hex"
          value={current}
          maxLength={7}
          onChange={(e) => onChange(e.target.value)}
        />
      </div>
      {schema.description && <p className="hint">{schema.description}</p>}
    </div>
  );
}
