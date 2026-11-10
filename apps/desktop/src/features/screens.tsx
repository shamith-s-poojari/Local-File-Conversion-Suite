import { useEffect, useMemo, useState } from "react";
import type {
  Api,
  AdapterEntry,
  Diagnostics,
  JobSnapshot,
  OperationTarget,
  Registry,
  Warning,
} from "../api";
import type { Context, Event, FileView } from "../machines/app";
import { fileIds } from "../machines/app";
import { errorMessage, formatBytes, SNIFF_ERRORS, TIER_LABELS, warningText } from "../text";
import { Thumbnail } from "../widgets/Thumbnail";
import { Password } from "../widgets/Password";
import { defaults, missingRequired, SchemaForm } from "./SchemaForm";

type Send = (e: Event) => void;

const TOOLS: [string, string][] = [
  ["Merge", "Join PDFs into one, in the order you choose."],
  ["Split", "Break a PDF into parts by page ranges, every N pages, or size."],
  ["Organize", "Reorder, rotate, delete or duplicate pages."],
  ["Protect or unlock", "Add or remove a password (AES-256)."],
  ["Watermark", "Stamp text on chosen pages."],
  ["Redact", "Remove text and areas for good, then verify they're gone."],
];

// ------------------------------------------------------------------ home

export function Home({ api, send, diag }: { api: Api; send: Send; diag: Diagnostics | null }) {
  const [busy, setBusy] = useState(false);
  const disabled = diag !== null && !diag.processingEnabled;
  const choose = async () => {
    setBusy(true);
    try {
      const results = await api.pickFiles();
      if (results.length) send({ type: "FILES_ADDED", results });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="screen home" aria-labelledby="home-title">
      <h1 id="home-title">Local File Conversion Suite</h1>
      {disabled && (
        <div className="banner error" role="alert">
          <strong>{errorMessage("E_SANDBOX_UNAVAILABLE").what}</strong>{" "}
          {diag?.startupError ?? errorMessage("E_SANDBOX_UNAVAILABLE").why}
        </div>
      )}
      <div className="dropzone" aria-describedby="drop-help">
        <p id="drop-help">Drop PDF files anywhere in this window, or</p>
        <button type="button" className="primary" onClick={choose} disabled={busy || disabled}>
          {busy ? "Opening…" : "Choose files"}
        </button>
      </div>
      <h2>Tools</h2>
      <ul className="tools-list">
        {TOOLS.map(([name, text]) => (
          <li key={name}>
            <strong>{name}</strong>
            <span>{text}</span>
          </li>
        ))}
      </ul>
      <details className="privacy">
        <summary>Processed on your device</summary>
        <p>
          Files never leave this computer. Each job runs in a sandbox that can only see a private copy of its own files, has no
          network access, and is deleted when you save or discard the result. The app has no update checks, telemetry or
          crash uploads.
        </p>
      </details>
    </section>
  );
}

// ---------------------------------------------------------------- review

export function Review({ api, ctx, send, rejected }: { api: Api; ctx: Context; send: Send; rejected: boolean }) {
  return (
    <section className="screen" aria-labelledby="review-title">
      <h1 id="review-title">{rejected ? "These files can't be used" : "Check your files"}</h1>
      {ctx.error && <ErrorBox code={ctx.error.code} />}
      <ul className="file-list">
        {ctx.files.map((f, i) => (
          <FileRow key={f.file?.fileId ?? `x${i}`} api={api} f={f} send={send} />
        ))}
      </ul>
      <div className="actions">
        {rejected ? (
          <button type="button" onClick={() => send({ type: "ACK" })}>
            Back
          </button>
        ) : (
          <button type="button" onClick={() => send({ type: "ABANDON" })}>
            Start over
          </button>
        )}
      </div>
    </section>
  );
}

function FileRow({ api, f, send }: { api: Api; f: FileView; send: Send }) {
  const [password, setPassword] = useState<string | undefined>();
  const [problem, setProblem] = useState<string | null>(null);
  const id = f.file?.fileId;
  const act = async (fn: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await fn();
      send({ type: "RESOLVED" });
    } catch (e) {
      setProblem(errorMessage((e as { code?: string }).code ?? "E_INTERNAL").what);
    }
  };
  return (
    <li className="file-row">
      <div>
        <span className="name">{f.displayName}</span>
        <span className="meta">
          {f.file ? `${f.file.format.toUpperCase()} · ${formatBytes(f.file.sizeBytes)}` : ""}
          {f.inspect ? ` · ${f.inspect.pages} page${f.inspect.pages === 1 ? "" : "s"}` : ""}
        </span>
      </div>
      {f.error && <p className="hint error">{SNIFF_ERRORS[f.error] ?? describeFileError(f.error)}</p>}
      {f.file?.extensionMismatch && <p className="hint">The name says one type, but the content is {f.file.format.toUpperCase()}.</p>}
      {f.inspect?.warnings.map((w) => (
        <p key={w} className="hint warning">
          {warningText(w)}
        </p>
      ))}
      {f.needs === "password" && id && (
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            if (password) void act(() => api.submitPassword(id, password).finally(() => setPassword(undefined)));
          }}
        >
          <Password
            id={`pw-${id}`}
            label="Password to open this file"
            schema={{}}
            value={password}
            onChange={setPassword}
            ctx={{ files: [], pages: 0, reference: id, api }}
            params={{}}
            setParam={() => {}}
          />
          <button type="submit" disabled={!password}>
            Open
          </button>
        </form>
      )}
      {f.needs === "format" && id && f.file && (
        <div className="row">
          <span>This file is valid as more than one type. Treat it as:</span>
          {f.file.candidates.map((c) => (
            <button key={c} type="button" onClick={() => act(() => api.resolveFile(id, c))}>
              {c.toUpperCase()}
            </button>
          ))}
        </div>
      )}
      {f.needs === "download" && id && (
        <div className="row">
          <span>This file is stored online only. Opening it downloads it from your cloud service.</span>
          <button type="button" onClick={() => act(() => api.resolveFile(id, undefined, true))}>
            Download and open
          </button>
        </div>
      )}
      {problem && (
        <p className="hint error" role="alert">
          {problem}
        </p>
      )}
      {id && (
        <button type="button" className="link" onClick={() => send({ type: "REMOVE_FILE", fileId: id })}>
          Remove
        </button>
      )}
    </li>
  );
}

function describeFileError(code: string): string {
  if (code === "notSupportedYet") return "Only PDF files can be processed in this version.";
  return errorMessage(code).what;
}

// ------------------------------------------------------------- configure

const NOTICES: Record<string, string[]> = {
  "pdf-protect": [
    "Permissions (print, copy, edit) are honored voluntarily by PDF viewers. They are not real access control.",
    "A lost password can't be recovered.",
  ],
  "pdf-watermark": ["A watermark is a label. It doesn't stop anyone from removing it."],
  "pdf-redact": [
    "Redaction also removes all comments, form fields, attachments, hidden layers, document properties and bookmarks that contain a term.",
    "The result is checked independently before you can save it. If checks fail, no file is produced.",
  ],
};

export function Configure({
  api,
  ctx,
  send,
  registry,
}: {
  api: Api;
  ctx: Context;
  send: Send;
  registry: Registry | null;
}) {
  const ids = fileIds(ctx);
  const [targets, setTargets] = useState<OperationTarget[] | null>(null);
  const [adding, setAdding] = useState(false);
  useEffect(() => {
    let live = true;
    api
      .getTargets(ids)
      .then((t) => live && setTargets(t.operations))
      .catch(() => live && setTargets([]));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, ids.join(",")]);

  const adapter: AdapterEntry | undefined = registry?.adapters.find((a) => a.manifest.id === ctx.operation);
  const first = ctx.files.find((f) => f.file?.actionable);
  const widgetCtx = useMemo(
    () => ({ files: ctx.files, pages: first?.inspect?.pages ?? 0, reference: first?.file?.fileId ?? "", api }),
    [ctx.files, first, api],
  );
  const missing = adapter ? missingRequired(adapter.paramsSchema, ctx.params) : ["operation"];
  const encrypted = ctx.files.some((f) => f.inspect && f.inspect.encryption !== "none");
  const visible = (targets ?? []).filter((t) => ctx.showBestEffort || !t.bestEffort);
  const invalid = ctx.error?.issues?.map((i) => i.path) ?? [];

  const addMore = async () => {
    setAdding(true);
    try {
      const results = await api.pickFiles();
      if (results.length) send({ type: "FILES_ADDED", results });
    } finally {
      setAdding(false);
    }
  };

  return (
    <section className="screen configure" aria-labelledby="configure-title">
      <h1 id="configure-title">Choose a tool</h1>
      <ul className="file-list compact">
        {ctx.files.map((f, i) => (
          <li key={f.file?.fileId ?? `x${i}`}>
            <span className="name">{f.displayName}</span>
            {f.inspect && <span className="meta">{f.inspect.pages} pages</span>}
            {f.file && (
              <button type="button" className="link" onClick={() => send({ type: "REMOVE_FILE", fileId: f.file!.fileId })}>
                Remove
              </button>
            )}
          </li>
        ))}
      </ul>
      <button type="button" onClick={addMore} disabled={adding}>
        Add files
      </button>

      <fieldset className="tool-choice">
        <legend>Tool</legend>
        {targets === null && <p className="hint">Finding tools…</p>}
        {visible.map((t) => {
          const reason = t.unavailable ? unavailableText(t.unavailable) : null;
          return (
            <label key={t.adapter} className={reason ? "disabled" : undefined}>
              <input
                type="radio"
                name="tool"
                value={t.adapter}
                checked={ctx.operation === t.adapter}
                disabled={Boolean(reason)}
                onChange={() => {
                  const a = registry?.adapters.find((x) => x.manifest.id === t.adapter);
                  send({ type: "SELECT_OPERATION", operation: t.adapter, params: a ? defaults(a.paramsSchema) : {} });
                }}
              />
              <span>{t.title}</span>
              <span className={`badge tier-${t.tier}`}>{TIER_LABELS[t.tier]}</span>
              {reason && <span className="hint">{reason}</span>}
            </label>
          );
        })}
        {(targets ?? []).some((t) => t.bestEffort) && (
          <label className="check">
            <input type="checkbox" checked={ctx.showBestEffort} onChange={() => send({ type: "TOGGLE_BEST_EFFORT" })} />
            Show best-effort tools
          </label>
        )}
      </fieldset>

      {adapter && (
        <>
          <h2>{adapter.manifest.title}</h2>
          {(NOTICES[adapter.manifest.id] ?? []).map((n) => (
            <p key={n} className="notice">
              {n}
            </p>
          ))}
          {encrypted && adapter.manifest.id !== "pdf-protect" && (
            <p className="notice">The result won't be password-protected.</p>
          )}
          <SchemaForm
            schema={adapter.paramsSchema}
            params={ctx.params}
            onChange={(params) => send({ type: "SET_PARAMS", params })}
            ctx={widgetCtx}
            invalid={invalid}
          />
        </>
      )}
      {ctx.error && <ErrorBox code={ctx.error.code} />}
      <div className="actions">
        <button type="button" onClick={() => send({ type: "CLEAR" })}>
          Start over
        </button>
        <button type="button" className="primary" disabled={missing.length > 0} onClick={() => send({ type: "START" })}>
          Start
        </button>
      </div>
    </section>
  );
}

function unavailableText(u: unknown): string {
  const o = (typeof u === "object" && u !== null ? u : {}) as {
    reason?: string;
    min?: number;
    max?: number;
  };
  switch (o.reason) {
    case "tooFewFiles":
      return `Needs at least ${o.min} files.`;
    case "tooManyFiles":
      return o.max === 1 ? "Works on one file at a time." : `Takes at most ${o.max} files.`;
    case "engineMissing":
      return "Not installed in this edition.";
    case "platformUnsupported":
      return "Not available on this system.";
    default:
      return "Not available.";
  }
}

// -------------------------------------------------------------- progress

export function ProgressScreen({ ctx, send, cancelling }: { ctx: Context; send: Send; cancelling: boolean }) {
  const job = ctx.job;
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, []);
  const p = job?.progress;
  const pct = p?.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
  return (
    <section className="screen" aria-labelledby="progress-title">
      <h1 id="progress-title">{cancelling ? "Cancelling…" : job?.state === "queued" || job?.state === "created" ? "Waiting to start…" : "Working…"}</h1>
      <div aria-live="polite" className="progress-box">
        <p>
          {job?.stage ? stageLabel(job.stage) : "Preparing"} · {elapsed}s
        </p>
        {pct === null ? (
          <div className="bar indeterminate" role="progressbar" aria-label="In progress" />
        ) : (
          <progress max={100} value={pct} aria-label={`${pct}%`} />
        )}
      </div>
      {job && job.stages.length > 0 && (
        <ol className="stage-log">
          {job.stages.map((s, i) => (
            <li key={`${i}-${s}`}>{stageLabel(s)}</li>
          ))}
        </ol>
      )}
      <Warnings warnings={job?.warnings ?? []} />
      <div className="actions">
        <button type="button" onClick={() => send({ type: "CANCEL" })} disabled={cancelling}>
          Cancel
        </button>
      </div>
    </section>
  );
}

function stageLabel(stage: string): string {
  if (stage.startsWith("verified-")) return `Passed check ${stage.slice(9)}`;
  return stage.charAt(0).toUpperCase() + stage.slice(1);
}

// ---------------------------------------------------------------- result

export function Result({ api, ctx, send, state }: { api: Api; ctx: Context; send: Send; state: string }) {
  const job = ctx.job;
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  if (!job) return null;
  const checks = job.stages.filter((s) => s.startsWith("verified-"));
  const busy = state === "saving" || state === "confirming";
  return (
    <section className="screen result" aria-labelledby="result-title">
      <h1 id="result-title">{job.state === "partial" ? "Partly done" : "Done"}</h1>
      {checks.length > 0 && (
        <div className="verified">
          <strong>Passed these checks:</strong> {checks.map((c) => c.slice(9)).join(", ")}. These checks look for recoverable
          content; they are not a guarantee.
        </div>
      )}
      <Warnings warnings={job.warnings} prominent />
      <ul className="outputs">
        {job.outputs.map((o, i) => (
          <li key={o.path}>
            {o.extension === "pdf" && (
              <Thumbnail api={api} reference={`${job.jobId}:${i}`} page={1} dpi={72} alt={`First page of ${o.suggestedName}`} />
            )}
            <span className="name">{o.suggestedName}</span>
            <span className="meta">{formatBytes(o.sizeBytes)}</span>
          </li>
        ))}
      </ul>
      {ctx.save?.status === "confirmOverwrite" && (
        <div className="banner warning" role="alert">
          <p>You chose one of the original files. Replace it with the result?</p>
          <button type="button" onClick={() => send({ type: "CONFIRM_OVERWRITE" })}>
            Replace the original
          </button>
        </div>
      )}
      {ctx.error && <ErrorBox code={ctx.error.code} />}
      <div className="actions">
        {confirmDiscard ? (
          <>
            <span>Discard the result? This can't be undone.</span>
            <button type="button" onClick={() => send({ type: "DISCARD_CONFIRMED" })}>
              Discard
            </button>
            <button type="button" onClick={() => setConfirmDiscard(false)}>
              Keep
            </button>
          </>
        ) : (
          <>
            <button type="button" onClick={() => setConfirmDiscard(true)}>
              Discard
            </button>
            <button type="button" onClick={() => send({ type: "ADJUST" })}>
              Adjust
            </button>
            <button type="button" className="primary" onClick={() => send({ type: "SAVE" })} disabled={busy}>
              {busy ? "Saving…" : job.outputs.length > 1 ? "Save all…" : "Save…"}
            </button>
          </>
        )}
      </div>
    </section>
  );
}

export function Failed({ ctx, send }: { ctx: Context; send: Send }) {
  const code = ctx.job?.error?.code ?? "E_INTERNAL";
  const m = errorMessage(code);
  const fallback = code === "E_REDACTION_VERIFICATION_FAILED" && ctx.params.rasterizeFallback !== true;
  return (
    <section className="screen" aria-labelledby="failed-title">
      <h1 id="failed-title">{m.what}</h1>
      {m.why && <p>{m.why}</p>}
      {m.todo && <p>{m.todo}</p>}
      <Warnings warnings={ctx.job?.warnings ?? []} />
      <div className="actions">
        <button type="button" onClick={() => send({ type: "DISMISS" })}>
          Start over
        </button>
        {fallback && (
          <button
            type="button"
            onClick={() => {
              send({ type: "RETRY" });
              send({ type: "SET_PARAMS", params: { ...ctx.params, rasterizeFallback: true } });
            }}
          >
            Use images for affected pages
          </button>
        )}
        <button type="button" className="primary" onClick={() => send({ type: "RETRY" })}>
          Change settings
        </button>
      </div>
    </section>
  );
}

export function Cancelled({ send }: { send: Send }) {
  return (
    <section className="screen" aria-labelledby="cancelled-title">
      <h1 id="cancelled-title">Cancelled</h1>
      <p>Nothing was saved, and the job's temporary files were deleted.</p>
      <div className="actions">
        <button type="button" className="primary" onClick={() => send({ type: "EDIT" })}>
          Back to settings
        </button>
      </div>
    </section>
  );
}

export function Saved({ ctx, send }: { ctx: Context; send: Send }) {
  const n = ctx.save?.count ?? 1;
  return (
    <section className="screen" aria-labelledby="saved-title">
      <h1 id="saved-title">Saved {n === 1 ? "" : `${n} files`}</h1>
      {ctx.save?.syncProvider && <p className="notice">This folder syncs to the cloud.</p>}
      <p>The job's temporary files were deleted.</p>
      <div className="actions">
        <button type="button" className="primary" onClick={() => send({ type: "NEW" })}>
          Convert another
        </button>
      </div>
    </section>
  );
}

// -------------------------------------------------------------- settings

export function Settings({ diag, onClose }: { diag: Diagnostics | null; onClose: () => void }) {
  return (
    <section className="screen" aria-labelledby="settings-title">
      <h1 id="settings-title">Settings and diagnostics</h1>
      {!diag ? (
        <p>Loading…</p>
      ) : (
        <>
          {diag.startupError && <ErrorBox code="E_INSTALL_CORRUPT" detail={diag.startupError} />}
          <dl className="facts">
            <dt>Version</dt>
            <dd>{diag.appVersion ?? "unknown"}</dd>
            <dt>Worker protocol</dt>
            <dd>{diag.protocolVersion ?? "unknown"}</dd>
            <dt>Processing</dt>
            <dd>{diag.processingEnabled ? "Enabled" : "Disabled"}</dd>
            <dt>Tools</dt>
            <dd>{(diag.adapters ?? []).join(", ")}</dd>
          </dl>
          <h2>Sandbox self-test</h2>
          {diag.sandbox ? (
            <table className="selftest">
              <thead>
                <tr>
                  <th>Check</th>
                  <th>Expected</th>
                  <th>Result</th>
                </tr>
              </thead>
              <tbody>
                {diag.sandbox.attempts.map((a) => (
                  <tr key={a.name} className={a.passed ? "" : "fail"}>
                    <td>{a.name.replace(/_/g, " ")}</td>
                    <td>{a.expected}</td>
                    <td>{a.passed ? "Pass" : `Fail (${a.outcome})`}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p>Running…</p>
          )}
        </>
      )}
      <h2>License</h2>
      <p>
        This app is free software under the GNU Affero General Public License v3.0. Source code:
        github.com/shamith-s-poojari/Local-File-Conversion-Suite
      </p>
      <div className="actions">
        <button type="button" onClick={onClose}>
          Back
        </button>
      </div>
    </section>
  );
}

// ----------------------------------------------------------------- parts

function ErrorBox({ code, detail }: { code: string; detail?: string }) {
  const m = errorMessage(code);
  return (
    <div className="banner error" role="alert">
      <strong>{m.what}</strong>
      {m.why && <p>{m.why}</p>}
      {m.todo && <p>{m.todo}</p>}
      {detail && <p className="hint">{detail}</p>}
    </div>
  );
}

function Warnings({ warnings, prominent }: { warnings: Warning[]; prominent?: boolean }) {
  if (warnings.length === 0) return null;
  const codes = [...new Set(warnings.map((w) => w.code))];
  return (
    <ul className={prominent ? "warnings prominent" : "warnings"} aria-label="Warnings">
      {codes.map((c) => (
        <li key={c}>
          <span className="badge warn">Warning</span> {warningText(c)}
        </li>
      ))}
    </ul>
  );
}

export type { JobSnapshot };
