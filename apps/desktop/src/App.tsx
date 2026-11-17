import { useMachine } from "@xstate/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { fromPromise } from "xstate";
import { api as realApi, events, isCommandError, type Api, type Diagnostics, type Format, type Registry } from "./api";
import { usePaste } from "./features/paste";
import { Cancelled, Configure, Failed, Home, ProgressScreen, Result, Review, Saved, Settings } from "./features/screens";
import { GearIcon, WarningIcon } from "./icons";
import { FINISH_FIRST } from "./text";
import { appMachine, type FileView } from "./machines/app";

/** Inspects files four at a time: each starts a sandboxed worker. */
export async function inspectAll(api: Api, files: FileView[]): Promise<FileView[]> {
  const out = [...files];
  let next = 0;
  const one = async (i: number) => {
    const f = files[i]!;
    if (!f.file) return;
    // Already inspected (files were added or one was removed since): each
    // inspection starts a sandboxed worker, so don't repeat it.
    if (f.inspect && !f.needs) return;
    if (f.file.cloudPlaceholder) {
      out[i] = { ...f, needs: "download" };
      return;
    }
    if (f.file.candidates.length > 0) {
      out[i] = { ...f, needs: "format" };
      return;
    }
    if (!f.file.actionable) {
      out[i] = { ...f, error: f.error ?? "notSupportedYet" };
      return;
    }
    try {
      const info = await api.inspectFile(f.file.fileId);
      // Without its password a file can't be opened, so inspect can only say
      // that it needs one (0 pages). Ask for it.
      const locked = info.encryption === "openPassword" && info.pages === 0;
      out[i] = locked
        ? { ...f, needs: "password", error: undefined }
        : { ...f, inspect: info, needs: undefined, error: undefined, reason: undefined };
    } catch (e) {
      const code = isCommandError(e) ? e.code : "E_INTERNAL";
      const reason = isCommandError(e) ? e.reason : undefined;
      if (code === "E_SANDBOX_UNAVAILABLE") throw e;
      // An Office file's password can't be given to the app: the prompt
      // would only fail, so say what to do instead.
      out[i] =
        code === "E_PASSWORD_REQUIRED" && reason !== "officePassword"
          ? { ...f, needs: "password" }
          : { ...f, error: code, reason, needs: undefined };
    }
  };
  const workers = Array.from({ length: Math.min(4, files.length) }, async () => {
    while (next < files.length) await one(next++);
  });
  await Promise.all(workers);
  return out;
}

export { looksTabular, readPaste } from "./features/paste";

/** Which screen a machine state shows. Related states share one, so focus
 * moves to the heading when the screen changes and not within it (audit U11). */
export function screenOf(state: string, settings: boolean): string {
  if (settings) return "settings";
  switch (state) {
    case "needsInput":
    case "rejected":
      return "review";
    case "configuring":
    case "preflight":
      return "configure";
    case "queued":
    case "processing":
    case "cancelling":
      return "progress";
    default:
      return state;
  }
}

export function App({ api = realApi }: { api?: Api }) {
  const machine = useMemo(
    () =>
      appMachine.provide({
        actors: {
          inspectAll: fromPromise(({ input }: { input: { files: FileView[] } }) => inspectAll(api, input.files)),
          createJob: fromPromise(
            async ({ input }: { input: { operation: string; to: Format | null; fileIds: string[]; params: Record<string, unknown> } }) =>
              input.to
                ? api.createConversionSkipping(input.fileIds, input.to, input.params)
                : { jobId: await api.createJob(input.operation, input.fileIds, input.params), skipped: 0 },
          ),
          save: fromPromise(
            ({ input }: { input: { jobId: string; confirm: boolean; keepLayout?: boolean; token?: string } }) =>
              input.confirm
                ? api.confirmSave(input.jobId, input.token ?? "")
                : api.saveOutputs(input.jobId, input.keepLayout ?? false),
          ),
          snapshotOutputs: fromPromise(async ({ input }: { input: { jobId: string } }) => {
            const s = await api.getSnapshot();
            return s.jobs.find((j) => j.jobId === input.jobId)?.outputs ?? [];
          }),
        },
        actions: {
          cancelJob: (_, p) => void api.cancelJob(p.jobId).catch(() => {}),
          discardJob: (_, p) => void api.discardJob(p.jobId).catch(() => {}),
          cancelSave: (_, p) => void api.cancelSave(p.jobId, p.token).catch(() => {}),
          removeFile: (_, p) => void api.removeFile(p.fileId).catch(() => {}),
          clearFiles: (_, p) => p.fileIds.forEach((id) => void api.removeFile(id).catch(() => {})),
        },
      }),
    [api],
  );
  const [state, send] = useMachine(machine);
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [registry, setRegistry] = useState<Registry | null>(null);
  const [settings, setSettings] = useState(false);
  const [dragging, setDragging] = useState(false);
  const acceptsPaste = !settings && (state.matches("idle") || state.matches("configuring"));
  const paste = usePaste(api, acceptsPaste, send);

  useEffect(() => {
    const unlisten = [
      events.onJob((event) => send({ type: "JOB", event })),
      events.onFilesAdded((results) => {
        if (!results.length) return;
        setSettings(false); // the files land on a screen, not behind Settings
        send({ type: "FILES_ADDED", results });
      }),
      events.onDragState(setDragging),
      events.onSelfTest(() => void api.getDiagnostics().then(setDiag)),
    ];
    api.getDiagnostics().then(setDiag).catch(() => {});
    api.getRegistry().then(setRegistry).catch(() => {});
    // Reload recovery (spec 11.2): Rust's job store is authoritative.
    api
      .getSnapshot()
      .then((s) => {
        const job = s.jobs.find((j) => !["succeeded", "partial", "failed", "cancelled"].includes(j.state)) ?? s.jobs[s.jobs.length - 1];
        if (job) send({ type: "RESTORE", job, seq: s.seq });
      })
      .catch(() => {});
    return () => {
      unlisten.forEach((p) => void p.then((f) => f()));
    };
  }, [api, send]);

  const ctx = state.context;
  const name = typeof state.value === "string" ? state.value : Object.keys(state.value)[0]!;
  const screenKey = screenOf(name, settings);
  // Focus the heading when the screen changes, not on first load.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    document.querySelector<HTMLElement>("#main h1")?.focus();
  }, [screenKey]);

  let screen;
  if (settings) {
    screen = <Settings diag={diag} registry={registry} onClose={() => setSettings(false)} />;
  } else if (state.matches("idle")) {
    screen = <Home api={api} send={send} diag={diag} dragging={dragging} />;
  } else if (state.matches("inspecting")) {
    screen = (
      <section className="screen narrow state-screen" aria-live="polite">
        <span className="state-icon spinning" aria-hidden="true" />
        <h1 tabIndex={-1}>Checking files…</h1>
        <p className="lede">Each file is opened in its own sandbox to read its size, pages or length.</p>
      </section>
    );
  } else if (state.matches("needsInput") || state.matches("rejected")) {
    screen = <Review api={api} ctx={ctx} send={send} rejected={state.matches("rejected")} />;
  } else if (state.matches("configuring") || state.matches("preflight")) {
    screen = <Configure api={api} ctx={ctx} send={send} registry={registry} diag={diag} busy={state.matches("preflight")} />;
  } else if (state.matches("queued") || state.matches("processing") || state.matches("cancelling")) {
    screen = <ProgressScreen ctx={ctx} send={send} cancelling={state.matches("cancelling")} />;
  } else if (state.matches("preview")) {
    const sub = typeof state.value === "object" ? String(Object.values(state.value)[0]) : "ready";
    screen = <Result api={api} ctx={ctx} send={send} state={sub} />;
  } else if (state.matches("failed")) {
    screen = <Failed api={api} ctx={ctx} send={send} />;
  } else if (state.matches("cancelled")) {
    screen = <Cancelled send={send} />;
  } else {
    screen = <Saved api={api} ctx={ctx} send={send} />;
  }

  return (
    <div className="app">
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <header className="topbar">
        <span className="brand">
          <span className="brand-mark" aria-hidden="true">
            <span />
            <span />
          </span>
          Local File Conversion Suite
        </span>
        <span className="local">
          <span className="dot on" aria-hidden="true" /> Processed on your device
        </span>
        <button type="button" className="ghost sm" onClick={() => setSettings((s) => !s)} aria-pressed={settings}>
          <GearIcon /> Settings
        </button>
      </header>
      {ctx.notice && (
        <div className="banner warning paste-problem" role="alert">
          <WarningIcon />
          <div>
            <p>{FINISH_FIRST}</p>
          </div>
          <button type="button" className="ghost sm" onClick={() => send({ type: "DISMISS_NOTICE" })}>
            Dismiss
          </button>
        </div>
      )}
      {paste.problem && (
        <div className="banner error paste-problem" role="alert">
          <WarningIcon />
          <div>
            <p>{paste.problem}</p>
          </div>
          <button type="button" className="ghost sm" onClick={paste.dismiss}>
            Dismiss
          </button>
        </div>
      )}
      <main id="main">{screen}</main>
    </div>
  );
}
