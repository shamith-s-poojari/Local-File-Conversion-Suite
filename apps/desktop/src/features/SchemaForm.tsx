import { useState, type ReactElement } from "react";
import type { JsonSchema } from "../api";
import { CONFIRM_SUFFIX } from "../machines/app";
import { CONFIRM_LABEL, FIELD_ERRORS, NEEDS } from "../text";
import { describedBy, parseLines, sameLines } from "../widgets/describe";
import { FileOrder } from "../widgets/FileOrder";
import { PageOrganizer } from "../widgets/PageOrganizer";
import { parsePageRanges, RANGE_TEXT } from "../widgets/pageRanges";
import { PageRangesInput } from "../widgets/PageRangesInput";
import { ColorInput, HEX } from "../widgets/ColorInput";
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

const empty = (v: unknown) => v === undefined || v === "";

/** A password the form must have: shown, and not marked optional in its
 * title. The schema can't say "required when mode is protect", so this is how
 * pdf-protect's password is required (audit U9). Such a password is typed twice. */
function needsPassword(s: JsonSchema): boolean {
  return s["x-ui"] === "password" && !/optional/i.test(s.title ?? "");
}

/** Client-side checks for START; the core validates again, authoritatively.
 * Returns the params that are missing or that a widget knows are invalid. */
export function missingRequired(
  schema: JsonSchema,
  params: Record<string, unknown>,
  pages = 0,
): string[] {
  const props = schema.properties ?? {};
  // A field that's hidden by `x-when` can't be filled in, so it isn't missing.
  const shown = (k: string) => !props[k]?.["x-when"] || matches(props[k]["x-when"], params, props);
  const missing = (schema.required ?? []).filter((k) => shown(k) && empty(params[k]));
  for (const [k, s] of Object.entries(props)) {
    if (!shown(k)) continue;
    const v = params[k];
    if (s["x-ui"] === "pageRanges" && typeof v === "string" && v.trim() && pages > 0) {
      if (!parsePageRanges(v, pages).ok) missing.push(k);
    }
    if (s["x-ui"] === "color" && typeof v === "string" && !HEX.test(v)) missing.push(k);
    if (needsPassword(s) && !missing.includes(k)) {
      if (empty(v)) missing.push(k);
      else if (params[k + CONFIRM_SUFFIX] !== v) missing.push(k + CONFIRM_SUFFIX);
    }
  }
  return missing;
}

/** What `missingRequired` found, in words for "Needs: …". */
export function missingLabels(schema: JsonSchema, params: Record<string, unknown>, pages = 0): string[] {
  const props = schema.properties ?? {};
  return missingRequired(schema, params, pages).map((k) => {
    if (k.endsWith(CONFIRM_SUFFIX) && props[k.slice(0, -CONFIRM_SUFFIX.length)]) {
      return empty(params[k]) ? NEEDS.confirmPassword : NEEDS.passwordsMatch;
    }
    const label = props[k]?.title ?? humanize(k);
    return /^[A-Z][a-z]/.test(label) ? label.charAt(0).toLowerCase() + label.slice(1) : label;
  });
}

/** The message under a field the core rejected: as specific as the schema allows. */
function fieldErrorText(s: JsonSchema): string {
  if (s["x-ui"] === "color") return FIELD_ERRORS.color;
  if (s["x-ui"] === "password") return FIELD_ERRORS.password;
  if (s["x-ui"] === "pageRanges") return RANGE_TEXT.syntax;
  if (s.enum) return FIELD_ERRORS.choice;
  const type = Array.isArray(s.type) ? s.type[0] : s.type;
  if (type === "integer" || type === "number") {
    const { minimum: lo, maximum: hi } = s;
    if (lo !== undefined && hi !== undefined) return FIELD_ERRORS.numberBetween(lo, hi);
    if (lo !== undefined) return FIELD_ERRORS.numberAtLeast(lo);
    if (hi !== undefined) return FIELD_ERRORS.numberAtMost(hi);
    return FIELD_ERRORS.number;
  }
  if (type === "string") {
    const { minLength: lo, maxLength: hi } = s;
    if (lo !== undefined && hi !== undefined) return FIELD_ERRORS.textBetween(lo, hi);
    if (lo !== undefined) return FIELD_ERRORS.textAtLeast(lo);
    if (hi !== undefined) return FIELD_ERRORS.textAtMost(hi);
    return FIELD_ERRORS.text;
  }
  return FIELD_ERRORS.generic;
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
        const rejected = invalid.some((p) => p === `/${key}` || p.startsWith(`/${key}/`));
        // A colour is checked here too: the core would only reject it on Start.
        const badColor = s["x-ui"] === "color" && typeof params[key] === "string" && !HEX.test(params[key]);
        const error = rejected || badColor;
        const errorId = `${id}-error`;
        const widget = s["x-ui"] ? WIDGETS[s["x-ui"]] : undefined;
        const confirmKey = key + CONFIRM_SUFFIX;
        const confirm = widget === Password && needsPassword(s);
        const mismatch = confirm && !empty(params[confirmKey]) && params[confirmKey] !== params[key];
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
              invalid={error}
              errorId={errorId}
            />
          );
        } else {
          field = (
            <Plain
              id={id}
              label={label}
              schema={s}
              value={params[key]}
              onChange={(v) => setParam(key, v)}
              invalid={error}
              errorId={errorId}
              required={schema.required?.includes(key)}
            />
          );
        }
        return (
          <div key={key} className={error ? "invalid" : undefined}>
            {field}
            {error && (
              <p id={errorId} className="hint error" role="alert">
                {fieldErrorText(s)}
              </p>
            )}
            {confirm && (
              <>
                <Password
                  id={`${id}-confirm`}
                  label={CONFIRM_LABEL}
                  schema={{}}
                  value={params[confirmKey] as string | undefined}
                  onChange={(v) => setParam(confirmKey, v)}
                  ctx={ctx}
                  params={params}
                  setParam={setParam}
                  invalid={mismatch}
                  errorId={`${id}-confirm-error`}
                />
                {mismatch && (
                  <p id={`${id}-confirm-error`} className="hint error" role="alert">
                    {FIELD_ERRORS.mismatch}
                  </p>
                )}
              </>
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
  invalid = false,
  errorId,
  required = false,
}: {
  id: string;
  label: string;
  schema: JsonSchema;
  value: unknown;
  onChange: (v: unknown) => void;
  invalid?: boolean;
  errorId?: string;
  required?: boolean;
}) {
  // What makes a field's problem reach a screen reader (audit U27).
  // The hint and the error are both read with the field.
  const hintId = `${id}-hint`;
  const a11y = {
    "aria-invalid": invalid || undefined,
    "aria-describedby": describedBy(schema.description && hintId, invalid && errorId),
    "aria-required": required || undefined,
  };
  const type = Array.isArray(schema.type) ? schema.type[0] : schema.type;
  const hint = schema.description ? (
    <p id={hintId} className="hint">
      {schema.description}
    </p>
  ) : null;
  if (schema.enum) {
    return (
      <div className="field">
        <label htmlFor={id}>{label}</label>
        <select id={id} {...a11y} value={String(value ?? schema.default ?? "")} onChange={(e) => {
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
        <input id={id} {...a11y} type="checkbox" checked={Boolean(value ?? schema.default)} onChange={(e) => onChange(e.target.checked)} />
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
          {...a11y}
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
      <fieldset className="field" id={id} aria-describedby={a11y["aria-describedby"]}>
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
    return <LinesField id={id} label={label} value={value as string[] | undefined} onChange={onChange} a11y={a11y} />;
  }
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        {...a11y}
        value={String(value ?? "")}
        maxLength={schema.maxLength}
        onChange={(e) => onChange(e.target.value === "" ? undefined : e.target.value)}
      />
      {hint}
    </div>
  );
}

/** A list of text, one item per line. The box keeps exactly what is typed
 * (so Enter makes a new line, and a blank line stays while you type the next
 * one); the list it stands for leaves out blank lines. Before this the box was
 * rebuilt from the list on every key, which swallowed each Enter. */
function LinesField({
  id,
  label,
  value,
  onChange,
  a11y,
}: {
  id: string;
  label: string;
  value: string[] | undefined;
  onChange: (v: unknown) => void;
  a11y: Record<string, unknown>;
}) {
  const [typed, setTyped] = useState(() => (value ?? []).join("\n"));
  // The list changed from outside (a reset, another tool): show it.
  const text = sameLines(typed, value) ? typed : (value ?? []).join("\n");
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <textarea
        id={id}
        {...a11y}
        aria-describedby={describedBy(a11y["aria-describedby"] as string | undefined, `${id}-lines-hint`)}
        rows={3}
        value={text}
        onChange={(e) => {
          setTyped(e.target.value);
          onChange(parseLines(e.target.value));
        }}
      />
      <p id={`${id}-lines-hint`} className="hint">
        One per line.
      </p>
    </div>
  );
}
