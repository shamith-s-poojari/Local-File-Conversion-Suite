import { useEffect, useMemo, useRef, useState } from "react";
import type { AdapterEntry, Api, ConversionTarget, Diagnostics, Format, OperationTarget, Registry, Targets } from "../../api";
import { FAMILIES, familyOf, formatInfo, TIER_TEXT, type Family } from "../../formats";
import { ArrowRight, CheckIcon, CloseIcon, PlusIcon } from "../../icons";
import type { Context } from "../../machines/app";
import { fileIds, startOverNeedsConfirm } from "../../machines/app";
import { isAction, suggest, type Suggestion } from "../../suggestions";
import { CONFIRM, ENGINES_IN_SETTINGS, NEEDS, REGISTRY_FAILED, STARTING, TARGETS_FAILED, TARGETS_RETRY } from "../../text";
import { RadioGroup } from "../radio";
import { usePicker } from "../pickers";
import { defaults, missingLabels, SchemaForm } from "../SchemaForm";
import { loadTargets } from "../targets";
import { ConfirmBar, ErrorBox, FilePreview, FileProblem, FormatTag, fileMeta, Reveals, SettingsLink, useReturnFocus, type Send } from "./shared";

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
  "file-clean": "Remove location, camera, author, dates and comments. Pictures don't change.",
  "media-clean": "Remove location, device, dates and tags without re-encoding.",
  "file-disarm": "Remove macros, scripts, embedded files and anything that loads from outside.",
  "contact-sheet": "Thumbnails of pictures or PDF pages on one sheet, with names.",
};

/** Tool names on the home screen, by the family whose files they take (ADR 0004). */
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

export function Configure({
  api,
  ctx,
  send,
  registry,
  diag = null,
  busy = false,
  registryFailed = false,
  onRetryRegistry,
}: {
  api: Api;
  ctx: Context;
  send: Send;
  registry: Registry | null;
  /** Which engines this computer has, for the install hint. */
  diag?: Diagnostics | null;
  /** The job is being checked and created (the machine's `preflight`). */
  busy?: boolean;
  /** The tool list couldn't be loaded: say so and offer to try again (audit UX-10). */
  registryFailed?: boolean;
  onRetryRegistry?: () => void;
}) {
  const ids = fileIds(ctx);
  // One array per set of files, so the effects below run when the files
  // change and not on every render.
  const idKey = ids.join(",");
  const idList = useMemo(() => (idKey ? idKey.split(",") : []), [idKey]);
  const [targets, setTargets] = useState<Targets | null>(null);
  const [targetsError, setTargetsError] = useState<string | null>(null);
  const picker = usePicker(api, send);
  const [attempt, setAttempt] = useState(0);
  const [confirmingStartOver, setConfirmingStartOver] = useState(false);
  const startOverButton = useRef<HTMLButtonElement>(null);
  useReturnFocus(confirmingStartOver, startOverButton);
  useEffect(() => {
    let live = true;
    setTargetsError(null);
    void loadTargets(api, idList).then((r) => {
      if (!live) return;
      setTargets(r.targets);
      setTargetsError(r.error);
    });
    return () => {
      live = false;
    };
  }, [api, idList, attempt]);
  const [duplicates, setDuplicates] = useState(0);
  useEffect(() => {
    let live = true;
    setDuplicates(0);
    if (idList.length > 1) {
      Promise.resolve()
        .then(() => api.findDuplicates(idList))
        .then((n) => live && setDuplicates(n))
        .catch(() => {});
    }
    return () => {
      live = false;
    };
  }, [api, idList]);

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
  // What Convert is waiting for, in words (audit U9).
  const needs: string[] = [];
  if (ids.length === 0) needs.push(NEEDS.file);
  if (!mixedAdapters) {
    if (adapter) needs.push(...missingLabels(adapter.paramsSchema, formParams, widgetCtx.pages));
    else needs.push(ctx.operation ? NEEDS.loading : NEEDS.choice);
  }
  const engineMissing = [...(targets?.operations ?? []), ...list0(targets)].some(
    (t) => typeof t.unavailable === "object" && t.unavailable !== null && (t.unavailable as { reason?: string }).reason === "engineMissing",
  );
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

  const chooseConversion = (c: ConversionTarget) => {
    const [id, ...others] = adaptersFor(c.to, targets);
    if (!id) return;
    const a = registry?.adapters.find((x) => x.manifest.id === id);
    // Several adapters: each part runs with its own defaults.
    const params = a && others.length === 0 ? defaults(a.paramsSchema) : {};
    send({ type: "SELECT_CONVERSION", to: c.to, adapter: id, params });
  };
  const chooseOperation = (t: OperationTarget, extra: Record<string, unknown> = {}) => {
    const a = registry?.adapters.find((x) => x.manifest.id === t.adapter);
    send({ type: "SELECT_OPERATION", operation: t.adapter, params: { ...(a ? defaults(a.paramsSchema, true) : {}), ...extra } });
  };
  const suggestions = suggest(ctx.files, targets, duplicates);
  const chooseSuggestion = (s: Suggestion) => {
    if ("to" in s) {
      const c = conversions.find((x) => x.to === s.to);
      if (c) chooseConversion(c);
      return;
    }
    const t = targets?.operations.find((o) => o.adapter === s.operation);
    if (t) chooseOperation(t, s.params);
  };
  const suggestionChecked = (s: Suggestion) =>
    "to" in s
      ? ctx.to === s.to
      : !ctx.to && ctx.operation === s.operation && Object.entries(s.params).every(([k, v]) => ctx.params[k] === v);
  // "Convert all to JPG" with some JPGs already selected: offer to leave them out.
  const sameFormat = ctx.to ? ctx.files.filter((f) => f.file?.actionable && f.file.format === ctx.to).length : 0;
  const offerSkip = sameFormat > 0 && sameFormat < ids.length;

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
    <section className="screen configure" aria-labelledby="configure-title" aria-busy={busy || undefined}>
      <header className="screen-head reveal">
        <p className="eyebrow">
          {ids.length} file{ids.length === 1 ? "" : "s"} selected
        </p>
        <h1 id="configure-title" tabIndex={-1}>
          Choose what to make
        </h1>
      </header>

      <div className="workspace">
        <aside className="files-panel" aria-label="Selected files">
          <div className="panel-head">
            <h2>Files</h2>
            <button type="button" className="ghost sm" onClick={() => picker.choose("files")} disabled={picker.busy}>
              <PlusIcon /> Add
            </button>
          </div>
          {picker.error && (
            <p className="hint error" role="alert">
              {picker.error}
            </p>
          )}
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
                  {f.file?.folder && <span className="hint folder-path">{f.file.folder}</span>}
                  <FileProblem f={f} />
                  <Reveals f={f} />
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
          {suggestions.length > 0 && (
            <section className="choice-section suggestions" aria-labelledby="suggested">
              <h2 id="suggested" className="section-title">
                Suggested
              </h2>
              <div className="chips">
                {suggestions.map((s) =>
                  isAction(s) ? (
                    <button
                      key={s.id}
                      type="button"
                      className={`chip${suggestionChecked(s) ? " checked" : ""}`}
                      aria-pressed={suggestionChecked(s)}
                      onClick={() => chooseSuggestion(s)}
                    >
                      {s.text}
                    </button>
                  ) : (
                    <span key={s.id} className="chip note">
                      {s.text}
                    </span>
                  ),
                )}
              </div>
            </section>
          )}
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
            {targetsError && (
              <div role="alert">
                <p className="hint error">
                  {TARGETS_FAILED} {targetsError}
                </p>
                <button type="button" className="sm" onClick={() => setAttempt((n) => n + 1)}>
                  {TARGETS_RETRY}
                </button>
              </div>
            )}
            {engineMissing && (
              <p className="hint">
                {ENGINES_IN_SETTINGS} <SettingsLink />
              </p>
            )}
            {targets !== null && !targetsError && conversions.length === 0 && (
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
                  <RadioGroup
                    className="target-grid"
                    label={FAMILIES.find((f) => f.id === fam)?.label ?? ""}
                    items={byFamily.get(fam)!.map((c) => ({ key: c.to, c, checked: ctx.to === c.to, disabled: Boolean(c.unavailable) }))}
                    render={({ c, checked }, tabIndex) => (
                      <TargetTile key={c.to} c={c} checked={checked} tabIndex={tabIndex} onChoose={() => chooseConversion(c)} />
                    )}
                  />
                </div>
              ))}
          </section>

          {operations.length > 0 && (
            <section className="choice-section" aria-labelledby="tools">
              <h2 id="tools" className="section-title">
                Tools
              </h2>
              <RadioGroup
                className="tool-list"
                label="Tools"
                items={operations.map((t) => ({
                  key: t.adapter,
                  t,
                  reason: t.unavailable ? unavailableText(t.unavailable) : null,
                  checked: !ctx.to && ctx.operation === t.adapter,
                  disabled: Boolean(t.unavailable),
                }))}
                render={({ t, reason, checked }, tabIndex) => (
                  <button
                    key={t.adapter}
                    type="button"
                    role="radio"
                    aria-checked={checked}
                    tabIndex={tabIndex}
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
                )}
              />
            </section>
          )}

          {offerSkip && (
            <label className="toggle">
              <input
                type="checkbox"
                checked={ctx.params.skipSame === true}
                onChange={() => send({ type: "SET_PARAMS", params: { ...ctx.params, skipSame: ctx.params.skipSame !== true } })}
              />
              <span>
                Skip {sameFormat} file{sameFormat === 1 ? "" : "s"} already in {formatInfo(ctx.to!).label}
              </span>
            </label>
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
          {registryFailed && !registry && (
            <div className="banner error" role="alert">
              <div>
                <strong>{REGISTRY_FAILED.what}</strong>
                <p>{REGISTRY_FAILED.todo}</p>
                {onRetryRegistry && (
                  <button type="button" className="sm" onClick={onRetryRegistry}>
                    {REGISTRY_FAILED.retry}
                  </button>
                )}
              </div>
            </div>
          )}
          {ctx.error && <ErrorBox code={ctx.error.code} reason={ctx.error.reason} diag={diag} />}
        </div>
      </div>

      <footer className="action-bar">
        <p className="summary">
          {summaryTarget && (
            <>
              <span className="mono">{ids.length}</span> file{ids.length === 1 ? "" : "s"} <ArrowRight /> <strong>{summaryTarget}</strong>
            </>
          )}
          {needs.length > 0 && (
            <span id="needs-note" className="needs">
              {NEEDS.prefix} {needs.join(", ")}
            </span>
          )}
        </p>
        {busy ? (
          <button type="button" onClick={() => send({ type: "CANCEL" })}>
            {STARTING.cancel}
          </button>
        ) : confirmingStartOver ? (
          <ConfirmBar
            message={CONFIRM.startOver.message}
            confirm={CONFIRM.startOver.confirm}
            keep={CONFIRM.startOver.keep}
            onConfirm={() => send({ type: "CLEAR" })}
            onKeep={() => setConfirmingStartOver(false)}
          />
        ) : (
          <button
            type="button"
            ref={startOverButton}
            className="ghost"
            onClick={() => (startOverNeedsConfirm(ctx) ? setConfirmingStartOver(true) : send({ type: "CLEAR" }))}
          >
            Start over
          </button>
        )}
        {/* aria-disabled, not disabled: focus stays on the button while it
            waits, and a screen reader can reach what it's waiting for. */}
        <button
          type="button"
          className="primary"
          aria-disabled={needs.length > 0 || busy || confirmingStartOver || undefined}
          aria-describedby={needs.length > 0 ? "needs-note" : undefined}
          onClick={() => {
            if (needs.length === 0 && !busy && !confirmingStartOver) send({ type: "START" });
          }}
        >
          <span>{busy ? STARTING.busy : ctx.to ? "Convert" : "Start"}</span>
          <span className="btn-icon">
            <ArrowRight />
          </span>
        </button>
      </footer>
    </section>
  );
}

export function TargetTile({
  c,
  checked,
  tabIndex,
  onChoose,
}: {
  c: ConversionTarget;
  checked: boolean;
  /** 0 for the one radio in its group that Tab reaches, -1 for the rest. */
  tabIndex: 0 | -1;
  onChoose: () => void;
}) {
  const info = formatInfo(c.to);
  const reason = c.unavailable ? unavailableText(c.unavailable) : null;
  const tier = TIER_TEXT[c.tier];
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      tabIndex={tabIndex}
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

function list0(t: Targets | null): ConversionTarget[] {
  return conversionChoices(t).list;
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

