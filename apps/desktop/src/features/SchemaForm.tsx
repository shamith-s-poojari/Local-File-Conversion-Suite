import type { ReactElement } from "react";
import type { JsonSchema } from "../api";
import { FileOrder } from "../widgets/FileOrder";
import { PageOrganizer } from "../widgets/PageOrganizer";
import { parsePageRanges } from "../widgets/pageRanges";
import { PageRangesInput } from "../widgets/PageRangesInput";
import { ColorInput } from "../widgets/ColorInput";
import { Password } from "../widgets/Password";
import { RedactionCanvas } from "../widgets/RedactionCanvas";
import { Slider } from "../widgets/Slider";
import type { WidgetContext, WidgetProps } from "../widgets/types";

// Forms are generated from each adapter's params schema, with named custom
// widgets for what a schema can't express (spec change C6).

// Each widget types its own value; the form passes whatever the param holds.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyWidget = (p: WidgetProps<any>) => ReactElement;

const WIDGETS: Record<string, AnyWidget> = {
  pageRanges: PageRangesInput,
  fileOrder: FileOrder,
  pageOrganizer: PageOrganizer,
  redactionCanvas: RedactionCanvas,
  password: Password,
  slider: Slider,
  color: ColorInput,
};

/** Never shown: filled by the core, or only set by a later user decision. */
const HIDDEN = new Set(["fileTitles", "rasterizeFallback"]);

/** Whether a param stays out of the form. A conversion's `to` is chosen in
 * the target picker; a tool's `to` (joining songs into one MP3 or FLAC,
 * ADR 0004) is an ordinary field, so tools pass `showTo`. */
function hidden(key: string, showTo: boolean): boolean {
  return HIDDEN.has(key) || (key === "to" && !showTo);
}

export function defaults(schema: JsonSchema, showTo = false): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, s] of Object.entries(schema.properties ?? {})) {
    if (s.default !== undefined && !hidden(k, showTo)) out[k] = s.default;
  }
  return out;
}

/** Client-side checks for START; the core validates again, authoritatively.
 * Returns the params that are missing or that a widget knows are invalid. */
export function missingRequired(
  schema: JsonSchema,
  params: Record<string, unknown>,
  pages = 0,
): string[] {
  const missing = (schema.required ?? []).filter((k) => params[k] === undefined || params[k] === "");
  for (const [k, s] of Object.entries(schema.properties ?? {})) {
    const v = params[k];
    if (s["x-ui"] === "pageRanges" && typeof v === "string" && v.trim() && pages > 0) {
      if (!parsePageRanges(v, pages).ok) missing.push(k);
    }
  }
  return missing;
}

/** `x-when`: every named param (or its default) has the given value, or one
 * of the values when a list is given. */
export function matches(
  when: Record<string, unknown>,
  params: Record<string, unknown>,
  props: Record<string, JsonSchema>,
): boolean {
  return Object.entries(when).every(([k, v]) => {
    const actual = params[k] ?? props[k]?.default;
    return Array.isArray(v) ? v.includes(actual) : actual === v;
  });
}

/** Visible fields: what the form shows for these params. */
export function visibleFields(
  schema: JsonSchema,
  params: Record<string, unknown>,
  showTo = false,
): string[] {
  const props = schema.properties ?? {};
  return Object.entries(props)
    .filter(([key, s]) => !hidden(key, showTo) && !s.readOnly && !(s.enum && s.enum.length === 1))
    .filter(([, s]) => !s["x-when"] || matches(s["x-when"], params, props))
    .map(([key]) => key);
}

function humanize(key: string): string {
  return key.replace(/([A-Z])/g, " $1").replace(/^./, (c) => c.toUpperCase());
}

export function SchemaForm({
  schema,
  params,
  onChange,
  ctx,
  invalid,
  showTo = false,
}: {
  schema: JsonSchema;
  params: Record<string, unknown>;
  onChange: (params: Record<string, unknown>) => void;
  ctx: WidgetContext;
  /** JSON pointers the core rejected, e.g. `/mode`. */
  invalid: string[];
  /** Show a `to` field: true for tools, false for conversions. */
  showTo?: boolean;
}) {
  const props = schema.properties ?? {};
  const hasCanvas = Object.values(props).some((p) => p["x-ui"] === "redactionCanvas");
  const setParam = (key: string, value: unknown) => {
    const next = { ...params };
    if (value === undefined) delete next[key];
    else next[key] = value;
    onChange(next);
  };
  return (
    <div className="form">
      {schema.description && <p className="hint">{schema.description}</p>}
      {Object.entries(props).map(([key, s]) => {
        if (hidden(key, showTo) || s.readOnly) return null;
        if (s["x-when"] && !matches(s["x-when"], params, props)) return null;
        if (s.enum && s.enum.length === 1) return null; // nothing to choose
        if (hasCanvas && key === "terms") return null; // edited inside the canvas
        const id = `param-${key}`;
        const label = s.title ?? humanize(key);
        const error = invalid.some((p) => p === `/${key}` || p.startsWith(`/${key}/`));
        const widget = s["x-ui"] ? WIDGETS[s["x-ui"]] : undefined;
        let field: ReactElement;
        if (widget) {
          const W = widget;
          field = (
            <W
              id={id}
              label={label}
              schema={s}
              value={params[key]}
              onChange={(v) => setParam(key, v)}
              ctx={ctx}
              params={params}
              setParam={setParam}
            />
          );
        } else {
          field = <Plain id={id} label={label} schema={s} value={params[key]} onChange={(v) => setParam(key, v)} />;
        }
        return (
          <div key={key} className={error ? "invalid" : undefined}>
            {field}
            {error && (
              <p className="hint error" role="alert">
                This value isn't valid.
              </p>
            )}
          </div>
        );
      })}
    </div>
  );
}

function Plain({
  id,
  label,
  schema,
  value,
  onChange,
}: {
  id: string;
  label: string;
  schema: JsonSchema;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  const type = Array.isArray(schema.type) ? schema.type[0] : schema.type;
  const hint = schema.description ? <p className="hint">{schema.description}</p> : null;
  if (schema.enum) {
    return (
      <div className="field">
        <label htmlFor={id}>{label}</label>
        <select id={id} value={String(value ?? schema.default ?? "")} onChange={(e) => {
          const raw = e.target.value;
          const match = schema.enum!.find((v) => String(v) === raw);
          onChange(match);
        }}>
          {schema.enum.map((v) => (
            <option key={String(v)} value={String(v)}>
              {schema["x-labels"]?.[String(v)] ?? humanize(String(v))}
            </option>
          ))}
        </select>
        {hint}
      </div>
    );
  }
  if (type === "boolean") {
    return (
      <div className="field check">
        <input id={id} type="checkbox" checked={Boolean(value ?? schema.default)} onChange={(e) => onChange(e.target.checked)} />
        <label htmlFor={id}>{label}</label>
        {hint}
      </div>
    );
  }
  if (type === "integer" || type === "number") {
    return (
      <div className="field">
        <label htmlFor={id}>{label}</label>
        <input
          id={id}
          type="number"
          min={schema.minimum}
          max={schema.maximum}
          step={type === "integer" ? 1 : "any"}
          value={value === undefined ? "" : String(value)}
          onChange={(e) => onChange(e.target.value === "" ? undefined : Number(e.target.value))}
        />
        {hint}
      </div>
    );
  }
  if (type === "object" && schema.properties) {
    const obj = (value as Record<string, unknown> | undefined) ?? {};
    return (
      <fieldset className="field" id={id}>
        <legend>{label}</legend>
        {Object.entries(schema.properties).map(([k, s]) => (
          <Plain
            key={k}
            id={`${id}-${k}`}
            label={s.title ?? humanize(k)}
            schema={s}
            value={obj[k]}
            onChange={(v) => onChange({ ...obj, [k]: v })}
          />
        ))}
        {hint}
      </fieldset>
    );
  }
  if (type === "array" && schema.items?.type === "string") {
    const list = (value as string[] | undefined) ?? [];
    return (
      <div className="field">
        <label htmlFor={id}>{label}</label>
        <textarea
          id={id}
          rows={3}
          value={list.join("\n")}
          onChange={(e) => {
            const items = e.target.value.split("\n").filter((x) => x.trim() !== "");
            onChange(items.length ? items : undefined);
          }}
        />
        <p className="hint">One per line.</p>
      </div>
    );
  }
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        value={String(value ?? "")}
        maxLength={schema.maxLength}
        onChange={(e) => onChange(e.target.value === "" ? undefined : e.target.value)}
      />
      {hint}
    </div>
  );
}
