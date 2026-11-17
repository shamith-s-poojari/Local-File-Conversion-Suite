import { useEffect, useState, type ReactElement } from "react";
import type { Api, Format, JobSnapshot } from "../../api";
import { familyOf, formatInfo } from "../../formats";
import { ArrowRight, ShieldIcon, WarningIcon } from "../../icons";
import type { Context, FileView } from "../../machines/app";
import { checkName, formatBytes, OVERWRITE_KEEP, skippedText } from "../../text";
import { Thumbnail } from "../../widgets/Thumbnail";
import { actionError } from "../actionError";
import { canPreview, ErrorBox, FamilyGlyph, FormatTag, Warnings, type Send } from "./shared";

// ----------------------------------------------------------------- result

export function Result({ api, ctx, send, state }: { api: Api; ctx: Context; send: Send; state: string }) {
  const job = ctx.job;
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const [layoutPossible, setLayoutPossible] = useState(false);
  const [keepLayout, setKeepLayout] = useState(true);
  const jobId = job?.jobId;
  const many = (job?.outputs.length ?? 0) > 1;
  useEffect(() => {
    let live = true;
    setLayoutPossible(false);
    if (jobId && many) {
      Promise.resolve()
        .then(() => api.canKeepLayout(jobId))
        .then((ok) => live && setLayoutPossible(ok))
        .catch(() => {});
    }
    return () => {
      live = false;
    };
  }, [api, jobId, many]);
  // Which file each "Partly done" number is: asked once, when it's needed.
  const [inputIds, setInputIds] = useState<string[] | null>(null);
  const partial = job?.state === "partial";
  useEffect(() => {
    let live = true;
    setInputIds(null);
    if (jobId && partial) {
      Promise.resolve()
        .then(() => api.jobInputs(jobId))
        .then((ids) => live && setInputIds(ids))
        .catch(() => {});
    }
    return () => {
      live = false;
    };
  }, [api, jobId, partial]);
  if (!job) return null;
  const checks = job.stages.filter((s) => s.startsWith("verified-"));
  const busy = state === "saving" || state === "confirming";
  const n = job.outputs.length;
  return (
    <section className="screen" aria-labelledby="result-title">
      <header className="screen-head reveal">
        <p className="eyebrow">{job.state === "partial" ? "Some files failed" : "Ready to save"}</p>
        <h1 id="result-title" tabIndex={-1}>
          {job.state === "partial" ? "Partly done" : "Done"}
        </h1>
        <p className="lede" role="status">
          {n} file{n === 1 ? " is" : "s are"} ready. Temporary copies are deleted once you save or discard.
        </p>
        {ctx.to && ctx.skipped > 0 && <p className="notice">{skippedText(ctx.skipped, formatInfo(ctx.to).label)}</p>}
      </header>
      {checks.length > 0 && (
        <div className="banner ok">
          <ShieldIcon />
          <div>
            <strong>Passed these checks:</strong>
            <ul className="check-list">
              {checks.map((c) => (
                <li key={c}>{checkName(c.slice(9))}</li>
              ))}
            </ul>
            <p>These checks look for recoverable content; they are not a guarantee.</p>
          </div>
        </div>
      )}
      <Warnings warnings={job.warnings} prominent inputNames={inputNames(inputIds, ctx.files)} />
      <ul className="output-grid">
        {job.outputs.map((o, i) => (
          <OutputCard key={o.path} api={api} jobId={job.jobId} index={i} o={o} />
        ))}
      </ul>
      {ctx.save?.status === "confirmOverwrite" && (
        <div className="banner warning" role="alert">
          <WarningIcon />
          <div>
            <p>You chose one of the original files. Replace it with the result?</p>
            <button type="button" onClick={() => send({ type: "CONFIRM_OVERWRITE" })}>
              Replace the original
            </button>
            <button type="button" className="ghost" onClick={() => send({ type: "DECLINE_OVERWRITE" })}>
              {OVERWRITE_KEEP}
            </button>
          </div>
        </div>
      )}
      {ctx.error && <ErrorBox code={ctx.error.code} reason={ctx.error.reason} load={() => api.getDiagnostics()} />}
      <footer className="action-bar">
        {confirmDiscard ? (
          <>
            <p className="summary">Discard the result? This can't be undone.</p>
            <button type="button" className="danger" onClick={() => send({ type: "DISCARD_CONFIRMED" })}>
              Discard
            </button>
            <button type="button" onClick={() => setConfirmDiscard(false)}>
              Keep
            </button>
          </>
        ) : (
          <>
            <p className="summary" />
            <button type="button" className="ghost" onClick={() => setConfirmDiscard(true)}>
              Discard
            </button>
            <button type="button" onClick={() => send({ type: "ADJUST" })}>
              Adjust
            </button>
            {layoutPossible && (
              <label className="toggle inline">
                <input type="checkbox" checked={keepLayout} onChange={() => setKeepLayout(!keepLayout)} />
                <span>Keep folder layout</span>
              </label>
            )}
            <button
              type="button"
              className="primary"
              onClick={() => send({ type: "SAVE", keepLayout: layoutPossible && keepLayout })}
              disabled={busy}
            >
              <span>{busy ? "Saving…" : n > 1 ? "Save all…" : "Save…"}</span>
              <span className="btn-icon">
                <ArrowRight />
              </span>
            </button>
          </>
        )}
      </footer>
    </section>
  );
}

/** The names of the job's inputs, in the order the core numbers them. The
 * core says which file each number is (`ids`, from `jobInputs`); a file the
 * app no longer lists gets "" and reads as "File N" (audit U8). */
export function inputNames(ids: string[] | null, files: FileView[]): string[] {
  const named = new Map(files.flatMap((f) => (f.file ? [[f.file.fileId, f.displayName] as const] : [])));
  return (ids ?? []).map((id) => named.get(id) ?? "");
}

/** Results the core can put on the clipboard: pictures (as an image) and text. */
const COPYABLE = [
  "png", "jpg", "webp", "gif", "bmp", "tiff", "avif", "heic", "svg", "ico",
  "txt", "md", "csv", "json", "yaml", "html",
];

function OutputCard({
  api,
  jobId,
  index,
  o,
}: {
  api: Api;
  jobId: string;
  index: number;
  o: JobSnapshot["outputs"][number];
}) {
  const [copy, setCopy] = useState<"idle" | "done" | string>("idle");
  const format = (o.extension || "unknown") as Format;
  const family = familyOf(format);
  const preview: ReactElement = canPreview(format) ? (
    <Thumbnail api={api} reference={`${jobId}:${index}`} page={1} dpi={72} alt={`Preview of ${o.suggestedName}`} fallback={<FamilyGlyph family={family} size="l" />} />
  ) : (
    <FamilyGlyph family={family} size="l" />
  );
  return (
    <li className="output-card">
      <div className="output-preview">{preview}</div>
      <div className="output-text">
        <span className="name" title={o.suggestedName}>
          {o.suggestedName}
        </span>
        <span className="meta">
          <FormatTag format={format} />
          <span className="mono">{formatBytes(o.sizeBytes)}</span>
        </span>
        {COPYABLE.includes(format) && (
          <button
            type="button"
            className="ghost sm"
            onClick={() =>
              api
                .copyOutput(jobId, index)
                .then(() => setCopy("done"))
                .catch((e) => setCopy(actionError(e)))
            }
          >
            {copy === "done" ? "Copied" : "Copy"}
          </button>
        )}
        {copy !== "idle" && copy !== "done" && (
          <span className="hint error" role="alert">
            {copy}
          </span>
        )}
      </div>
    </li>
  );
}

