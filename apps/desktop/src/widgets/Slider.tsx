import type { WidgetProps } from "./types";

/** A number on a range, with the value shown beside it (`x-ui: slider`). */
export function Slider({ id, label, schema, value, onChange }: WidgetProps<number>) {
  const min = schema.minimum ?? 0;
  const max = schema.maximum ?? 100;
  const current = typeof value === "number" ? value : typeof schema.default === "number" ? schema.default : min;
  return (
    <div className="field slider">
      <div className="slider-head">
        <label htmlFor={id}>{label}</label>
        <output htmlFor={id} className="mono">
          {current}
        </output>
      </div>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={1}
        value={current}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      {schema.description && <p className="hint">{schema.description}</p>}
    </div>
  );
}
