import { createContext, useContext, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Api, Diagnostics, Format, Warning } from "../../api";
import { describeMedia, familyOf, type Family } from "../../formats";
import { FAMILY_ICONS, WarningIcon } from "../../icons";
import type { Event, FileView } from "../../machines/app";
import {
  ACTIVE_KINDS,
  DIAGNOSTICS,
  ENGINE_NAMES,
  errorMessage,
  FINDING_LABELS,
  folderTruncatedText,
  formatBytes,
  installCommand,
  OPEN_SETTINGS,
  reasonMessage,
  SNIFF_ERRORS,
  UNKNOWN_TYPE,
  unnamedFile,
  warningDetailText,
  warningText,
} from "../../text";
import { Thumbnail } from "../../widgets/Thumbnail";
import { copyToClipboard, diagnosticsText, missingEngines } from "../diagnostics";

export type Send = (e: Event) => void;

/** Opens Settings. App provides it; a screen rendered alone has none, and the
 * link is left out. */
export const OpenSettingsContext = createContext<(() => void) | null>(null);

/** A link to Settings, for text that sends people there (audit UX-6). */
export function SettingsLink() {
  const open = useContext(OpenSettingsContext);
  if (!open) return null;
  return (
    <button type="button" className="link settings-link" onClick={open}>
      {OPEN_SETTINGS}
    </button>
  );
}

/** Ask before going on, inline: the question is announced, focus goes to the
 * safe answer, Escape means keep, and `onKeep` is where the caller puts focus
 * back on the button that asked. */
export function ConfirmBar({
  message,
  confirm,
  keep,
  onConfirm,
  onKeep,
  danger = false,
}: {
  message: ReactNode;
  confirm: string;
  keep: string;
  onConfirm: () => void;
  onKeep: () => void;
  danger?: boolean;
}) {
  const safe = useRef<HTMLButtonElement>(null);
  useEffect(() => safe.current?.focus(), []);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onKeep();
    }
  };
  return (
    <div className="confirm-bar" role="alertdialog" aria-label={confirm} aria-describedby="confirm-message" onKeyDown={onKeyDown}>
      <p id="confirm-message" className="summary">
        {message}
      </p>
      <button type="button" ref={safe} onClick={onKeep}>
        {keep}
      </button>
      <button type="button" className={danger ? "danger" : undefined} onClick={onConfirm}>
        {confirm}
      </button>
    </div>
  );
}

/** Focus goes back to `ref` when `open` turns from true to false, so a
 * keyboard user lands where they were after a confirmation closes. */
export function useReturnFocus(open: boolean, ref: { current: HTMLElement | null }) {
  const was = useRef(open);
  useEffect(() => {
    if (was.current && !open) ref.current?.focus();
    was.current = open;
  }, [open, ref]);
}

export const PREVIEWABLE: Family[] = ["image", "audio", "video"];

export function canPreview(format: Format): boolean {
  return format === "pdf" || PREVIEWABLE.includes(familyOf(format));
}

// ------------------------------------------------------------------ parts

export function FormatTag({ format }: { format: Format }) {
  if (format === "unknown") {
    return (
      <span className={`tag fam-${familyOf(format)}`}>
        <span aria-hidden="true">?</span>
        <span className="visually-hidden">{UNKNOWN_TYPE}</span>
      </span>
    );
  }
  return <span className={`tag fam-${familyOf(format)}`}>{format.toUpperCase()}</span>;
}

export function FamilyGlyph({ family, size = "m" }: { family: Family; size?: "s" | "m" | "l" }) {
  const Icon = FAMILY_ICONS[family];
  return (
    <span className={`glyph fam-${family} glyph-${size}`}>
      <Icon />
    </span>
  );
}

export function FilePreview({ api, f, size = "m" }: { api: Api; f: FileView; size?: "m" | "l" }) {
  const format = f.file?.format ?? "unknown";
  const family = familyOf(format);
  if (!f.file || !canPreview(format) || f.needs || f.error) {
    return <FamilyGlyph family={family} size={size === "l" ? "l" : "m"} />;
  }
  return (
    <Thumbnail
      api={api}
      reference={f.file.fileId}
      page={1}
      dpi={48}
      alt={`Preview of ${f.displayName}`}
      fallback={<FamilyGlyph family={family} />}
    />
  );
}

export function fileMeta(f: FileView): string {
  const parts: string[] = [];
  if (f.file) parts.push(formatBytes(f.file.sizeBytes));
  if (f.inspect?.media) {
    const m = describeMedia(f.inspect.media);
    if (m) parts.push(m);
  } else if (f.inspect && f.inspect.pages > 0 && f.file?.format === "pdf") {
    parts.push(`${f.inspect.pages} page${f.inspect.pages === 1 ? "" : "s"}`);
  }
  return parts.join(" · ");
}

export function ErrorBox({
  code,
  reason,
  detail,
  load,
  diag,
}: {
  code: string;
  /** The command's reason: its text is used before the code's (audit U1). */
  reason?: string;
  detail?: string;
  load?: () => Promise<Diagnostics>;
  /** For `engineMissing`: which engines are missing, and the system. */
  diag?: Diagnostics | null;
}) {
  const m = reasonMessage(code, reason);
  const missing = reason === "engineMissing" ? missingEngines(diag) : [];
  return (
    <div className="banner error" role="alert">
      <WarningIcon />
      <div>
        <strong>{m.what}</strong>
        {m.why && <p>{m.why}</p>}
        {m.todo && <p>{m.todo}</p>}
        {detail && <p className="hint">{detail}</p>}
        {missing.map((e) => {
          const cmd = installCommand(e, diag?.platform);
          return (
            <p key={e} className="hint">
              {ENGINE_NAMES[e]} is missing.{cmd && <> Install it with <code className="install-cmd">{cmd}</code></>}
            </p>
          );
        })}
        {(code === "E_SANDBOX_UNAVAILABLE" || reason === "engineMissing" || reason === "selfTestPending") && <SettingsLink />}
        {m.report && load && <CopyDiagnostics load={load} />}
      </div>
    </div>
  );
}

/** "Copy diagnostics" for a bug report. If the clipboard refuses, the text is
 * shown selected-ready instead, so copying by hand always works (audit U30). */
export function CopyDiagnostics({ load }: { load: () => Promise<Diagnostics> }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const [text, setText] = useState("");
  const copy = async () => {
    try {
      const t = diagnosticsText(await load());
      setText(t);
      setState((await copyToClipboard(t)) ? "copied" : "failed");
    } catch {
      setState("failed");
    }
  };
  return (
    <div className="copy-diagnostics">
      <button type="button" className="ghost sm" onClick={copy}>
        {state === "copied" ? DIAGNOSTICS.copied : DIAGNOSTICS.copy}
      </button>
      <span className="visually-hidden" role="status">
        {state === "copied" ? DIAGNOSTICS.copied : ""}
      </span>
      <span className="hint">{DIAGNOSTICS.note}</span>
      {state === "failed" && (
        <>
          <p className="hint error" role="alert">
            {DIAGNOSTICS.failed}
          </p>
          {text && (
            <textarea className="mono small" readOnly rows={8} aria-label={DIAGNOSTICS.copy} value={text} onFocus={(e) => e.currentTarget.select()} />
          )}
        </>
      )}
    </div>
  );
}

export function Warnings({
  warnings,
  prominent,
  inputNames,
}: {
  warnings: Warning[];
  prominent?: boolean;
  /** The job's input names, in input order, to say which files a part
   * failure was about (audit U8). */
  inputNames?: string[];
}) {
  if (warnings.length === 0) return null;
  const items: { key: string; text: string; detail: string | null }[] = [];
  const seen = new Set<string>();
  for (const w of warnings) {
    if (w.code === "PART_FAILED") {
      const part = partFailure(w.detail, inputNames ?? []);
      if (part) {
        items.push({ key: `p${items.length}`, text: part.names, detail: part.what });
        continue;
      }
    }
    if (seen.has(w.code)) continue;
    seen.add(w.code);
    items.push({ key: w.code, text: warningText(w.code), detail: warningDetailText(w.detail) });
  }
  return (
    <ul className={prominent ? "warnings prominent" : "warnings"} aria-label="Warnings">
      {items.map((it) => (
        <li key={it.key}>
          <WarningIcon />
          <span>
            {it.text}
            {it.detail && <span className="hint"> {it.detail}</span>}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** A PART_FAILED warning's detail (`{ code, inputs }`) as names and the
 * code's text, or null when it isn't in that shape. */
export function partFailure(detail: unknown, names: string[]): { names: string; what: string } | null {
  if (typeof detail !== "object" || detail === null) return null;
  const d = detail as { code?: unknown; inputs?: unknown };
  if (typeof d.code !== "string" || !Array.isArray(d.inputs) || !d.inputs.every((i) => Number.isInteger(i) && i >= 0)) return null;
  const files = (d.inputs as number[]).map((i) => names[i] || unnamedFile(i));
  const m = errorMessage(d.code);
  return { names: files.join(", "), what: [m.what, m.todo].filter(Boolean).join(" ") };
}

/** What a file gives away or could do when opened (ADR 0005). */
export function Reveals({ f }: { f: FileView }) {
  const findings = f.inspect?.findings ?? [];
  const macro = f.file?.macroEnabled === true;
  if (findings.length === 0 && !macro) return null;
  const kinds = [...new Set(findings.map((x) => x.kind))];
  const active = kinds.filter((k) => ACTIVE_KINDS.includes(k));
  const privacy = kinds.filter((k) => !ACTIVE_KINDS.includes(k));
  return (
    <details className="reveals">
      <summary>
        {(active.length > 0 || macro) && (
          <span className="caution">
            <WarningIcon /> Can run or load things when opened
          </span>
        )}
        {privacy.length > 0 && (
          <span className="reveal-line">Reveals: {privacy.map((k) => FINDING_LABELS[k] ?? k).join(", ")}</span>
        )}
      </summary>
      <ul>
        {macro && !kinds.includes("macros") && <li>Macro-enabled file</li>}
        {findings.map((x, i) => (
          <li key={i}>
            <span className="finding-kind">{FINDING_LABELS[x.kind] ?? x.kind}</span>
            {x.detail && <span className="mono">{x.detail}</span>}
          </li>
        ))}
      </ul>
      <p className="hint">Found on this device. Nothing was sent anywhere.</p>
    </details>
  );
}

export function FileProblem({ f }: { f: FileView }) {
  if (!f.error) return null;
  if (f.error === "folderTruncated") return <p className="hint warn-text">{folderTruncatedText(f.count)}</p>;
  const plain = SNIFF_ERRORS[f.error];
  if (plain) return <p className="hint error">{plain}</p>;
  if (f.error === "notSupportedYet") return <p className="hint error">{describeFileError(f.error)}</p>;
  // A command's error: what happened, why and what to do (audit U5).
  const m = reasonMessage(f.error, f.reason);
  return (
    <>
      <p className="hint error">{m.what}</p>
      {m.why && <p className="hint">{m.why}</p>}
      {m.todo && <p className="hint">{m.todo}</p>}
    </>
  );
}

export function describeFileError(code: string): string {
  if (code === "notSupportedYet") return "This type of file isn't supported.";
  return errorMessage(code).what;
}
