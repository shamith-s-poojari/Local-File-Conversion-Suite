import type { ReactElement } from "react";
import type { JsonSchema } from "../api";
import { FileOrder } from "../widgets/FileOrder";
import { PageOrganizer } from "../widgets/PageOrganizer";
import { PageRangesInput } from "../widgets/PageRangesInput";
import { Password } from "../widgets/Password";
import { RedactionCanvas } from "../widgets/RedactionCanvas";
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
};

/** Never shown: filled by the core, or only set by a later user decision. */
const HIDDEN = new Set(["fileTitles", "rasterizeFallback"]);

export function defaults(schema: JsonSchema): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, s] of Object.entries(schema.properties ?? {})) {
    if (s.default !== undefined && !HIDDEN.has(k)) out[k] = s.default;
  }
  return out;
}

/** Client-side checks for START; the core validates again, authoritatively. */
export function missingRequired(schema: JsonSchema, params: Record<string, unknown>): string[] {
  return (schema.required ?? []).filter((k) => params[k] === undefined || params[k] === "");
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
}: {
  schema: JsonSchema;
  params: Record<string, unknown>;
  onChange: (params: Record<string, unknown>) => void;
  ctx: WidgetContext;
  /** JSON pointers the core rejected, e.g. `/mode`. */
  invalid: string[];
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
        if (HIDDEN.has(key) || s.readOnly) return null;
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
              {humanize(String(v))}
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
