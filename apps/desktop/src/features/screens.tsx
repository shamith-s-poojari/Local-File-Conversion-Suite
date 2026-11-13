import { useEffect, useMemo, useState, type ReactElement } from "react";
import type {
  AdapterEntry,
  Api,
  ConversionTarget,
  Diagnostics,
  Format,
  JobSnapshot,
  OperationTarget,
  Registry,
  Targets,
  Warning,
} from "../api";
import { describeMedia, FAMILIES, familyOf, formatInfo, TIER_TEXT, type Family } from "../formats";
import {
  ArrowRight,
  CheckIcon,
  CloseIcon,
  FAMILY_ICONS,
  GearIcon,
  LockIcon,
  PlusIcon,
  ShieldIcon,
  ToolIcon,
  UploadIcon,
  WarningIcon,
} from "../icons";
import type { Context, Event, FileView } from "../machines/app";
import { fileIds } from "../machines/app";
import { errorMessage, formatBytes, installHint, SNIFF_ERRORS, warningText } from "../text";
import { Password } from "../widgets/Password";
import { Thumbnail } from "../widgets/Thumbnail";
import { defaults, missingRequired, SchemaForm } from "./SchemaForm";

type Send = (e: Event) => void;

const TOOLS: Record<string, string> = {
  "images-to-pdf": "Put the images into one PDF, one image per page.",
  "pdf-merge": "Join PDFs in the order you choose.",
  "pdf-split": "Break a PDF into parts by pages, every N pages, or size.",
  "pdf-organize": "Reorder, rotate, delete or duplicate pages.",
  "pdf-protect": "Add or remove a password (AES-256).",
  "pdf-watermark": "Stamp text across chosen pages.",
  "pdf-redact": "Remove text and areas for good, then check they're gone.",
  "pdf-compress": "Make the PDF smaller by packing it and shrinking its pictures.",
  "pdf-page-numbers": "Number the pages, with optional text such as a title.",
  "image-edit": "Rotate, flip, resize or turn black and white. Each keeps its format.",
  "image-compress": "Get each image under a size, such as 200 KB for an upload form.",
  "images-to-gif": "Turn pictures into an animated GIF, in the order you choose.",
  "audio-join": "Join songs or recordings into one file, in the order you choose.",
  "audio-edit": "Even out the loudness, change the volume, fade in or out, or change speed.",
  "video-join": "Join clips into one video, in the order you choose.",
  "video-edit": "Rotate, flip, speed up, slow down or mute. Each keeps its format.",
  "video-frames": "Save pictures from the video, spread evenly or every few seconds.",
  "data-combine": "Stack rows from several files into one, or put each on its own sheet.",
  "data-split": "Break a table into files of so many rows, or one per value in a column.",
};

/** Tool names on the home screen, by the family whose files they take (ADR 0004). */
const FAMILY_TOOLS: Partial<Record<Family, string[]>> = {
  image: ["Rotate and resize", "Compress", "Animated GIF", "Images to one PDF"],
  audio: ["Join", "Volume, fade and speed"],
  video: ["Join", "Rotate and speed", "Save frames"],
  data: ["Combine", "Split"],
};

const PDF_TOOLS = [
  "Merge",
  "Split",
  "Organize pages",
  "Compress",
  "Page numbers",
  "Protect or unlock",
  "Watermark",
  "Redact",
];

const PREVIEWABLE: Family[] = ["image", "audio", "video"];

function canPreview(format: Format): boolean {
  return format === "pdf" || PREVIEWABLE.includes(familyOf(format));
}

// ------------------------------------------------------------------ parts

function FormatTag({ format }: { format: Format }) {
  return <span className={`tag fam-${familyOf(format)}`}>{format === "unknown" ? "?" : format.toUpperCase()}</span>;
}

function FamilyGlyph({ family, size = "m" }: { family: Family; size?: "s" | "m" | "l" }) {
  const Icon = FAMILY_ICONS[family];
  return (
    <span className={`glyph fam-${family} glyph-${size}`}>
      <Icon />
    </span>
  );
}

function FilePreview({ api, f, size = "m" }: { api: Api; f: FileView; size?: "m" | "l" }) {
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

function fileMeta(f: FileView): string {
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

function ErrorBox({ code, detail }: { code: string; detail?: string }) {
  const m = errorMessage(code);
  return (
    <div className="banner error" role="alert">
      <WarningIcon />
      <div>
        <strong>{m.what}</strong>
        {m.why && <p>{m.why}</p>}
        {m.todo && <p>{m.todo}</p>}
        {detail && <p className="hint">{detail}</p>}
      </div>
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
          <WarningIcon />
          <span>{warningText(c)}</span>
        </li>
      ))}
    </ul>
  );
}

// ------------------------------------------------------------------- home

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
  const missing = missingEngines(diag);
  return (
    <section className="screen home" aria-labelledby="home-title">
      <header className="hero reveal">
        <p className="eyebrow">Local file converter</p>
        <h1 id="home-title" className="display">
          Convert any file, right on this computer.
        </h1>
        <p className="lede">
          Documents, images, audio and video. Each file opens in its own sandbox with no network access, and nothing is
          uploaded anywhere.
        </p>
      </header>
      {disabled && (
        <div className="banner error" role="alert">
          <WarningIcon />
          <div>
            <strong>{errorMessage("E_SANDBOX_UNAVAILABLE").what}</strong>
            <p>{diag?.startupError ?? errorMessage("E_SANDBOX_UNAVAILABLE").why}</p>
          </div>
        </div>
      )}
      <div className="bezel reveal">
        <div className="dropzone" aria-describedby="drop-help">
          <span className="drop-icon">
            <UploadIcon />
          </span>
          <p className="drop-title">Drop files anywhere in this window</p>
          <p id="drop-help" className="hint">
            Up to 200 at once. Mixed types are fine.
          </p>
          <button type="button" className="primary lg" onClick={choose} disabled={busy || disabled}>
            <span>{busy ? "Opening…" : "Choose files"}</span>
            <span className="btn-icon">
              <ArrowRight />
            </span>
          </button>
        </div>
      </div>

      <section className="bento" aria-label="What it converts">
        {FAMILIES.map((fam, i) => (
          <article key={fam.id} className={`tile tile-${fam.id} reveal`} data-index={i}>
            <FamilyGlyph family={fam.id} />
            <h2>{fam.label}</h2>
            <p>{fam.blurb}</p>
            {(fam.id === "audio" || fam.id === "video") && missing.includes("ffmpeg") && (
              <p className="hint warn-text">{installHint("ffmpeg", diag?.platform)}</p>
            )}
            {(fam.id === "document" || fam.id === "data") && missing.includes("libreoffice") && (
              <p className="hint warn-text">{installHint("libreoffice", diag?.platform)} Word, Excel and PowerPoint files use it.</p>
            )}
            {FAMILY_TOOLS[fam.id] && (
              <ul className="tool-names family-tools" aria-label={`${fam.label} tools`}>
                {FAMILY_TOOLS[fam.id]!.map((t) => (
                  <li key={t}>{t}</li>
                ))}
              </ul>
            )}
          </article>
        ))}
        <article className="tile tile-tools reveal">
          <span className="glyph glyph-m fam-tools">
            <ToolIcon />
          </span>
          <h2>PDF tools</h2>
          <ul className="tool-names">
            {PDF_TOOLS.map((t) => (
              <li key={t}>{t}</li>
            ))}
          </ul>
        </article>
      </section>

      <details className="privacy">
        <summary>
          <ShieldIcon /> Processed on your device
        </summary>
        <p>
          Files never leave this computer. Each job runs in a sandbox that can only see a private copy of its own files, has no
          network access, and is deleted when you save or discard the result. The app has no update checks, telemetry or crash
          uploads.
        </p>
      </details>
    </section>
  );
}

function missingEngines(diag: Diagnostics | null): string[] {
  if (!diag?.engines) return [];
  return ["ffmpeg", "libreoffice"].filter((e) => !diag.engines!.includes(e));
}

// ----------------------------------------------------------------- review

export function Review({ api, ctx, send, rejected }: { api: Api; ctx: Context; send: Send; rejected: boolean }) {
  return (
    <section className="screen narrow" aria-labelledby="review-title">
      <header className="screen-head reveal">
        <p className="eyebrow">{rejected ? "Nothing to convert" : "One more thing"}</p>
        <h1 id="review-title">{rejected ? "These files can't be used" : "Check your files"}</h1>
      </header>
      {ctx.error && <ErrorBox code={ctx.error.code} />}
      <ul className="file-cards">
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
    <li className="file-card">
      <div className="file-card-main">
        <FamilyGlyph family={familyOf(f.file?.format ?? "unknown")} />
        <div className="file-text">
          <span className="name">{f.displayName}</span>
          <span className="meta">
            {f.file && <FormatTag format={f.file.format} />} <span className="mono">{fileMeta(f)}</span>
          </span>
        </div>
        {id && (
          <button type="button" className="icon-btn" aria-label={`Remove ${f.displayName}`} onClick={() => send({ type: "REMOVE_FILE", fileId: id })}>
            <CloseIcon />
          </button>
        )}
      </div>
      {f.error && <p className="hint error">{SNIFF_ERRORS[f.error] ?? describeFileError(f.error)}</p>}
      {f.file?.extensionMismatch && (
        <p className="hint">The name says one type, but the content is {f.file.format.toUpperCase()}. It will be treated as {f.file.format.toUpperCase()}.</p>
      )}
      {f.inspect?.warnings.map((w) => (
        <p key={w} className="hint warn-text">
          {warningText(w)}
        </p>
      ))}
      {f.needs === "password" && id && (
        <form
          className="inline-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (password) void act(() => api.submitPassword(id, password).finally(() => setPassword(undefined)));
          }}
        >
          <LockIcon />
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
          <button type="submit" className="primary" disabled={!password}>
            Open
          </button>
        </form>
      )}
      {f.needs === "format" && id && f.file && (
        <div className="inline-form">
          <span>This file is valid as more than one type. Treat it as:</span>
          {f.file.candidates.map((c) => (
            <button key={c} type="button" onClick={() => act(() => api.resolveFile(id, c))}>
              {formatInfo(c).label}
            </button>
          ))}
        </div>
      )}
      {f.needs === "download" && id && (
        <div className="inline-form">
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
    </li>
  );
}

function describeFileError(code: string): string {
  if (code === "notSupportedYet") return "This type of file isn't supported.";
  return errorMessage(code).what;
}

// -------------------------------------------------------------- configure

const NOTICES: Record<string, string[]> = {
  "pdf-protect": [
    "Permissions (print, copy, edit) are honored voluntarily by PDF viewers. They are not real access control.",
    "A lost password can't be recovered.",
  ],
  "pdf-watermark": ["A watermark is a label. It doesn't stop anyone from removing it."],
  "pdf-page-numbers": ["Numbers use a standard font with Western European characters; others show as “?”."],
  "video-join": ["Clips that differ in size or format are re-encoded to match the first, which takes longer."],
  "video-edit": ["Each video is re-encoded once at high quality. Long videos take a while."],
  "pdf-redact": [
    "Redaction also removes all comments, form fields, attachments, hidden layers, document properties and bookmarks that contain a term.",
    "The result is checked independently before you can save it. If checks fail, no file is produced.",
  ],
};

/** Conversion targets for the selection: one group's, or the shared ones. */
function conversionChoices(t: Targets | null): { list: ConversionTarget[]; mixed: boolean } {
  if (!t) return { list: [], mixed: false };
  if (t.groups.length === 1) return { list: t.groups[0]!.conversions, mixed: false };
  return { list: t.shared, mixed: t.groups.length > 1 };
}

/**
 * The adapters converting the selection to `to`, one per kind of file. A
 * mixed selection ("all to PDF" for Word files and photos) can need several;
 * the job then runs one part per adapter, each with its default settings.
 */
export function adaptersFor(to: Format, t: Targets | null): string[] {
  const ids = new Set<string>();
  for (const g of t?.groups ?? []) {
    const id = g.conversions.find((x) => x.to === to)?.plan?.hops[0]?.adapter;
    if (id) ids.add(id);
  }
  return [...ids];
}

export function Configure({ api, ctx, send, registry }: { api: Api; ctx: Context; send: Send; registry: Registry | null }) {
  const ids = fileIds(ctx);
  const [targets, setTargets] = useState<Targets | null>(null);
  const [adding, setAdding] = useState(false);
  useEffect(() => {
    let live = true;
    api
      .getTargets(ids)
      .then((t) => live && setTargets(t))
      .catch(() => live && setTargets({ operations: [], groups: [], shared: [] }));
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
  // The target format is part of the params the core validates.
  const formParams = ctx.to ? { ...ctx.params, to: ctx.to } : ctx.params;
  const parts = ctx.to ? adaptersFor(ctx.to, targets) : [];
  const mixedAdapters = parts.length > 1;
  const missing = mixedAdapters
    ? []
    : adapter
      ? missingRequired(adapter.paramsSchema, formParams, widgetCtx.pages)
      : ["operation"];
  const encrypted = ctx.files.some((f) => f.inspect && f.inspect.encryption !== "none");
  const invalid = ctx.error?.issues?.map((i) => i.path) ?? [];
  const { list, mixed } = conversionChoices(targets);
  // A video with no sound track has no soundtrack to save.
  const silent = ctx.files.some((f) => f.inspect?.media?.kind === "video" && f.inspect.media.hasAudio === false);
  const conversions = list
    .filter((c) => ctx.showBestEffort || !c.bestEffort)
    .filter((c) => !(silent && familyOf(c.to) === "audio"));
  const operations = (targets?.operations ?? []).filter((t) => ctx.showBestEffort || !t.bestEffort);
  const hiddenBestEffort = [...list, ...(targets?.operations ?? [])].some((t) => t.bestEffort);

  const addMore = async () => {
    setAdding(true);
    try {
      const results = await api.pickFiles();
      if (results.length) send({ type: "FILES_ADDED", results });
    } finally {
      setAdding(false);
    }
  };

  const chooseConversion = (c: ConversionTarget) => {
    const [id, ...others] = adaptersFor(c.to, targets);
    if (!id) return;
    const a = registry?.adapters.find((x) => x.manifest.id === id);
    // Several adapters: each part runs with its own defaults.
    const params = a && others.length === 0 ? defaults(a.paramsSchema) : {};
    send({ type: "SELECT_CONVERSION", to: c.to, adapter: id, params });
  };
  const chooseOperation = (t: OperationTarget) => {
    const a = registry?.adapters.find((x) => x.manifest.id === t.adapter);
    send({ type: "SELECT_OPERATION", operation: t.adapter, params: a ? defaults(a.paramsSchema, true) : {} });
  };

  const byFamily = new Map<Family, ConversionTarget[]>();
  for (const c of conversions) {
    const fam = familyOf(c.to);
    byFamily.set(fam, [...(byFamily.get(fam) ?? []), c]);
  }
  // The selection's own family first: a video's video formats, then the rest.
  const own = ctx.files.find((f) => f.file)?.file?.format;
  const base: Family[] = ["document", "image", "audio", "video", "data"];
  const order: Family[] = own ? [familyOf(own), ...base.filter((f) => f !== familyOf(own))] : base;
  const summaryTarget = ctx.to ? formatInfo(ctx.to).label : adapter?.manifest.title;

  return (
    <section className="screen configure" aria-labelledby="configure-title">
      <header className="screen-head reveal">
        <p className="eyebrow">
          {ids.length} file{ids.length === 1 ? "" : "s"} selected
        </p>
        <h1 id="configure-title">Choose what to make</h1>
      </header>

      <div className="workspace">
        <aside className="files-panel" aria-label="Selected files">
          <div className="panel-head">
            <h2>Files</h2>
            <button type="button" className="ghost sm" onClick={addMore} disabled={adding}>
              <PlusIcon /> Add
            </button>
          </div>
          <ul className="file-list">
            {ctx.files.map((f, i) => (
              <li key={f.file?.fileId ?? `x${i}`} className="file-item">
                <FilePreview api={api} f={f} />
                <div className="file-text">
                  <span className="name" title={f.displayName}>
                    {f.displayName}
                  </span>
                  <span className="meta">
                    {f.file && <FormatTag format={f.file.format} />}
                    <span className="mono">{fileMeta(f)}</span>
                  </span>
                </div>
                {f.file && (
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`Remove ${f.displayName}`}
                    onClick={() => send({ type: "REMOVE_FILE", fileId: f.file!.fileId })}
                  >
                    <CloseIcon />
                  </button>
                )}
              </li>
            ))}
          </ul>
        </aside>

        <div className="choices">
          <section className="choice-section" aria-labelledby="convert-to">
            <h2 id="convert-to" className="section-title">
              {mixed ? "Convert all to" : "Convert to"}
            </h2>
            {targets === null && (
              <div className="skeleton-grid" aria-hidden="true">
                {Array.from({ length: 8 }, (_, i) => (
                  <span key={i} className="skeleton" />
                ))}
              </div>
            )}
            {targets !== null && conversions.length === 0 && (
              <p className="empty">
                {mixed
                  ? "These files have no format in common. Convert each kind separately, or remove some files."
                  : "Nothing to convert this file to yet."}
              </p>
            )}
            {order
              .filter((fam) => byFamily.has(fam))
              .map((fam) => (
                <div key={fam} className="target-group">
                  <h3 className="group-label">{FAMILIES.find((f) => f.id === fam)?.label}</h3>
                  <div className="target-grid" role="radiogroup" aria-label={FAMILIES.find((f) => f.id === fam)?.label}>
                    {byFamily.get(fam)!.map((c) => (
                      <TargetTile
                        key={c.to}
                        c={c}
                        checked={ctx.to === c.to}
                        onChoose={() => chooseConversion(c)}
                      />
                    ))}
                  </div>
                </div>
              ))}
          </section>

          {operations.length > 0 && (
            <section className="choice-section" aria-labelledby="tools">
              <h2 id="tools" className="section-title">
                Tools
              </h2>
              <div className="tool-list" role="radiogroup" aria-label="Tools">
                {operations.map((t) => {
                  const reason = t.unavailable ? unavailableText(t.unavailable) : null;
                  const checked = !ctx.to && ctx.operation === t.adapter;
                  return (
                    <button
                      key={t.adapter}
                      type="button"
                      role="radio"
                      aria-checked={checked}
                      className={`tool-row${checked ? " checked" : ""}`}
                      disabled={Boolean(reason)}
                      onClick={() => chooseOperation(t)}
                    >
                      <span className="tool-title">{t.title}</span>
                      <span className="tool-text">{reason ?? TOOLS[t.adapter] ?? ""}</span>
                      <span className="tick">
                        <CheckIcon />
                      </span>
                    </button>
                  );
                })}
              </div>
            </section>
          )}

          {hiddenBestEffort && (
            <label className="toggle">
              <input type="checkbox" checked={ctx.showBestEffort} onChange={() => send({ type: "TOGGLE_BEST_EFFORT" })} />
              <span>Show best-effort conversions</span>
              <span className="hint">These rebuild structure from layout; check the result.</span>
            </label>
          )}

          {mixedAdapters && (
            <section className="choice-section options" aria-labelledby="options-title">
              <h2 id="options-title" className="section-title">
                Options
              </h2>
              <p className="notice">
                These kinds of file convert differently, so each uses its usual settings. To change them, convert one kind
                at a time.
              </p>
              {encrypted && <p className="notice">The results won't be password-protected.</p>}
            </section>
          )}
          {adapter && !mixedAdapters && (
            <section className="choice-section options" aria-labelledby="options-title">
              <h2 id="options-title" className="section-title">
                Options
              </h2>
              {(NOTICES[adapter.manifest.id] ?? []).map((n) => (
                <p key={n} className="notice">
                  {n}
                </p>
              ))}
              {encrypted && adapter.manifest.id !== "pdf-protect" && <p className="notice">The result won't be password-protected.</p>}
              <SchemaForm
                schema={adapter.paramsSchema}
                params={formParams}
                onChange={(params) => {
                  const { to: _to, ...rest } = params;
                  send({ type: "SET_PARAMS", params: ctx.to ? rest : params });
                }}
                ctx={widgetCtx}
                invalid={invalid}
                showTo={!ctx.to}
              />
            </section>
          )}
          {ctx.error && <ErrorBox code={ctx.error.code} />}
        </div>
      </div>

      <footer className="action-bar">
        <p className="summary">
          {summaryTarget ? (
            <>
              <span className="mono">{ids.length}</span> file{ids.length === 1 ? "" : "s"} <ArrowRight /> <strong>{summaryTarget}</strong>
            </>
          ) : (
            "Pick a format or a tool."
          )}
        </p>
        <button type="button" className="ghost" onClick={() => send({ type: "CLEAR" })}>
          Start over
        </button>
        <button type="button" className="primary" disabled={missing.length > 0} onClick={() => send({ type: "START" })}>
          <span>{ctx.to ? "Convert" : "Start"}</span>
          <span className="btn-icon">
            <ArrowRight />
          </span>
        </button>
      </footer>
    </section>
  );
}

function TargetTile({ c, checked, onChoose }: { c: ConversionTarget; checked: boolean; onChoose: () => void }) {
  const info = formatInfo(c.to);
  const reason = c.unavailable ? unavailableText(c.unavailable) : null;
  const tier = TIER_TEXT[c.tier];
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      disabled={Boolean(reason)}
      className={`target fam-${info.family}${checked ? " checked" : ""}`}
      onClick={onChoose}
      title={reason ?? `${tier.label}. ${tier.help}`}
    >
      <span className="target-label">{info.label}</span>
      <span className="tick" aria-hidden="true">
        <CheckIcon />
      </span>
      <span className="target-note">{reason ?? info.note}</span>
      <span className={`tier tier-${c.tier}`}>{tier.label}</span>
    </button>
  );
}

function unavailableText(u: unknown): string {
  const o = (typeof u === "object" && u !== null ? u : {}) as { reason?: string; min?: number; max?: number; engine?: string };
  switch (o.reason) {
    case "tooFewFiles":
      return `Needs at least ${o.min} files.`;
    case "tooManyFiles":
      return o.max === 1 ? "Works on one file at a time." : `Takes at most ${o.max} files.`;
    case "engineMissing":
      return o.engine === "ffmpeg" ? "Needs FFmpeg" : o.engine === "libreoffice" ? "Needs LibreOffice" : "Not installed";
    case "platformUnsupported":
      return "Not available on this system.";
    default:
      return "Not available.";
  }
}

// --------------------------------------------------------------- progress

export function ProgressScreen({ ctx, send, cancelling }: { ctx: Context; send: Send; cancelling: boolean }) {
  const job = ctx.job;
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, []);
  const p = job?.progress;
  const pct = p?.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
  const waiting = job?.state === "queued" || job?.state === "created";
  const target = ctx.to ? formatInfo(ctx.to).label : null;
  return (
    <section className="screen narrow" aria-labelledby="progress-title">
      <header className="screen-head reveal">
        <p className="eyebrow">{target ? `Converting to ${target}` : "Working"}</p>
        <h1 id="progress-title">{cancelling ? "Cancelling…" : waiting ? "Waiting to start…" : "Working on it…"}</h1>
      </header>
      <div className="bezel">
        <div className="progress-card" aria-live="polite">
          <div className="progress-head">
            <span>{job?.stage ? stageLabel(job.stage) : "Preparing"}</span>
            <span className="mono">
              {pct === null ? "" : `${pct}% · `}
              {formatElapsed(elapsed)}
            </span>
          </div>
          {pct === null ? (
            <div className="bar indeterminate" role="progressbar" aria-label="In progress" />
          ) : (
            <div className="bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={`${pct}%`}>
              <ProgressFill pct={pct} />
            </div>
          )}
          {p?.total ? (
            <p className="hint mono">
              {p.done} of {p.total}
            </p>
          ) : null}
        </div>
      </div>
      {job && job.stages.length > 0 && (
        <ol className="stage-log">
          {job.stages.map((s, i) => (
            <li key={`${i}-${s}`} className={i < job.stages.length - 1 ? "done" : "current"}>
              <CheckIcon /> {stageLabel(s)}
            </li>
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

/** The bar's fill, sized through the CSSOM (inline style attributes are blocked by the CSP). */
function ProgressFill({ pct }: { pct: number }) {
  return <span className="bar-fill" ref={(el) => el?.style.setProperty("--pct", String(pct / 100))} />;
}

function formatElapsed(s: number): string {
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

function stageLabel(stage: string): string {
  if (stage.startsWith("verified-")) return `Passed check ${stage.slice(9)}`;
  const named: Record<string, string> = {
    convert: "Converting",
    render: "Rendering pages",
    assemble: "Assembling",
    save: "Saving",
    encode: "Encoding",
    decode: "Reading",
  };
  return named[stage] ?? stage.charAt(0).toUpperCase() + stage.slice(1);
}

// ----------------------------------------------------------------- result

export function Result({ api, ctx, send, state }: { api: Api; ctx: Context; send: Send; state: string }) {
  const job = ctx.job;
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  if (!job) return null;
  const checks = job.stages.filter((s) => s.startsWith("verified-"));
  const busy = state === "saving" || state === "confirming";
  const n = job.outputs.length;
  return (
    <section className="screen" aria-labelledby="result-title">
      <header className="screen-head reveal">
        <p className="eyebrow">{job.state === "partial" ? "Some files failed" : "Ready to save"}</p>
        <h1 id="result-title">{job.state === "partial" ? "Partly done" : "Done"}</h1>
        <p className="lede">
          {n} file{n === 1 ? " is" : "s are"} ready. Temporary copies are deleted once you save or discard.
        </p>
      </header>
      {checks.length > 0 && (
        <div className="banner ok">
          <ShieldIcon />
          <div>
            <strong>Passed these checks: {checks.map((c) => c.slice(9)).join(", ")}.</strong>
            <p>These checks look for recoverable content; they are not a guarantee.</p>
          </div>
        </div>
      )}
      <Warnings warnings={job.warnings} prominent />
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
          </div>
        </div>
      )}
      {ctx.error && <ErrorBox code={ctx.error.code} />}
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
            <button type="button" className="primary" onClick={() => send({ type: "SAVE" })} disabled={busy}>
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

function OutputCard({ api, jobId, index, o }: { api: Api; jobId: string; index: number; o: JobSnapshot["outputs"][number] }) {
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
      </div>
    </li>
  );
}

export function Failed({ ctx, send }: { ctx: Context; send: Send }) {
  const code = ctx.job?.error?.code ?? "E_INTERNAL";
  const m = errorMessage(code);
  const fallback = code === "E_REDACTION_VERIFICATION_FAILED" && ctx.params.rasterizeFallback !== true;
  return (
    <section className="screen narrow state-screen" aria-labelledby="failed-title">
      <span className="state-icon error">
        <WarningIcon />
      </span>
      <h1 id="failed-title">{m.what}</h1>
      {m.why && <p className="lede">{m.why}</p>}
      {m.todo && <p>{m.todo}</p>}
      <Warnings warnings={ctx.job?.warnings ?? []} />
      <div className="actions center">
        <button type="button" className="ghost" onClick={() => send({ type: "DISMISS" })}>
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
    <section className="screen narrow state-screen" aria-labelledby="cancelled-title">
      <span className="state-icon">
        <CloseIcon />
      </span>
      <h1 id="cancelled-title">Cancelled</h1>
      <p className="lede">Nothing was saved, and the job's temporary files were deleted.</p>
      <div className="actions center">
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
    <section className="screen narrow state-screen" aria-labelledby="saved-title">
      <span className="state-icon ok">
        <CheckIcon />
      </span>
      <h1 id="saved-title">{n === 1 ? "Saved" : `Saved ${n} files`}</h1>
      {ctx.save?.syncProvider && <p className="notice">This folder syncs to the cloud.</p>}
      <p className="lede">The job's temporary files were deleted.</p>
      <div className="actions center">
        <button type="button" className="primary" onClick={() => send({ type: "NEW" })}>
          <span>Convert more</span>
          <span className="btn-icon">
            <ArrowRight />
          </span>
        </button>
      </div>
    </section>
  );
}

// --------------------------------------------------------------- settings

const ENGINE_NAMES: [string, string, string][] = [
  ["ffmpeg", "FFmpeg", "Audio and video"],
  ["libreoffice", "LibreOffice", "Word, PowerPoint, Excel and OpenDocument"],
  ["pdf2docx", "pdf2docx", "PDF to Word"],
  ["pillow-heif", "pillow-heif", "iPhone HEIC photos"],
  ["markdown-it-py", "markdown-it-py", "Markdown"],
  ["openpyxl", "openpyxl", "Excel to and from CSV, JSON and YAML"],
  ["pyyaml", "PyYAML", "YAML files"],
];

export function Settings({ diag, onClose }: { diag: Diagnostics | null; onClose: () => void }) {
  return (
    <section className="screen narrow" aria-labelledby="settings-title">
      <header className="screen-head reveal">
        <p className="eyebrow">
          <GearIcon /> Settings
        </p>
        <h1 id="settings-title">Settings and diagnostics</h1>
      </header>
      {!diag ? (
        <p>Loading…</p>
      ) : (
        <>
          {diag.startupError && <ErrorBox code="E_INSTALL_CORRUPT" detail={diag.startupError} />}
          <dl className="facts card">
            <dt>Version</dt>
            <dd className="mono">{diag.appVersion ?? "unknown"}</dd>
            <dt>Worker protocol</dt>
            <dd className="mono">{diag.protocolVersion ?? "unknown"}</dd>
            <dt>Processing</dt>
            <dd>{diag.processingEnabled ? "Enabled" : "Disabled"}</dd>
            <dt>Tools</dt>
            <dd className="mono small">{(diag.adapters ?? []).join(", ")}</dd>
          </dl>
          <h2 className="section-title">Engines on this computer</h2>
          <ul className="engine-list card">
            {ENGINE_NAMES.map(([id, name, use]) => {
              const found = diag.engines?.includes(id);
              return (
                <li key={id}>
                  <span className={`dot ${found ? "on" : "off"}`} aria-hidden="true" />
                  <span className="engine-name">{name}</span>
                  <span className="hint">{use}</span>
                  <span className="engine-state">{found ? "Found" : "Not found"}</span>
                </li>
              );
            })}
          </ul>
          <h2 className="section-title">Sandbox self-test</h2>
          {diag.sandbox ? (
            <table className="selftest card">
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
                    <td className="mono">{a.expected}</td>
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
      <h2 className="section-title">License</h2>
      <p className="hint">
        Free software under the GNU Affero General Public License v3.0. Source code: github.com/shamith-s-poojari/Local-File-Conversion-Suite
      </p>
      <div className="actions">
        <button type="button" onClick={onClose}>
          Back
        </button>
      </div>
    </section>
  );
}

export type { JobSnapshot };
