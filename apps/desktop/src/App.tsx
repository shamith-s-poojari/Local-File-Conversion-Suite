import { useMachine } from "@xstate/react";
import { useEffect, useMemo, useState } from "react";
import { fromPromise } from "xstate";
import { api as realApi, events, isCommandError, type Api, type Diagnostics, type Registry } from "./api";
import { Cancelled, Configure, Failed, Home, ProgressScreen, Result, Review, Saved, Settings } from "./features/screens";
import { appMachine, type FileView } from "./machines/app";

/** Inspects files four at a time: each starts a sandboxed worker. */
export async function inspectAll(api: Api, files: FileView[]): Promise<FileView[]> {
  const out = [...files];
  let next = 0;
  const one = async (i: number) => {
    const f = files[i]!;
    if (!f.file) return;
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
        : { ...f, inspect: info, needs: undefined, error: undefined };
    } catch (e) {
      const code = isCommandError(e) ? e.code : "E_INTERNAL";
      if (code === "E_SANDBOX_UNAVAILABLE") throw e;
      out[i] = code === "E_PASSWORD_REQUIRED" ? { ...f, needs: "password" } : { ...f, error: code, needs: undefined };
    }
  };
  const workers = Array.from({ length: Math.min(4, files.length) }, async () => {
    while (next < files.length) await one(next++);
  });
  await Promise.all(workers);
  return out;
}

export function App({ api = realApi }: { api?: Api }) {
  const machine = useMemo(
    () =>
      appMachine.provide({
        actors: {
          inspectAll: fromPromise(({ input }: { input: { files: FileView[] } }) => inspectAll(api, input.files)),
          createJob: fromPromise(({ input }: { input: { operation: string; fileIds: string[]; params: Record<string, unknown> } }) =>
            api.createJob(input.operation, input.fileIds, input.params),
          ),
          save: fromPromise(({ input }: { input: { jobId: string; confirm: boolean } }) =>
            input.confirm ? api.confirmSave(input.jobId) : api.saveOutputs(input.jobId),
          ),
          snapshotOutputs: fromPromise(async ({ input }: { input: { jobId: string } }) => {
            const s = await api.getSnapshot();
            return s.jobs.find((j) => j.jobId === input.jobId)?.outputs ?? [];
          }),
        },
        actions: {
          cancelJob: (_, p) => void api.cancelJob(p.jobId).catch(() => {}),
          discardJob: (_, p) => void api.discardJob(p.jobId).catch(() => {}),
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

  useEffect(() => {
    const unlisten = [
      events.onJob((event) => send({ type: "JOB", event })),
      events.onFilesAdded((results) => results.length && send({ type: "FILES_ADDED", results })),
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
  let screen;
  if (settings) {
    screen = <Settings diag={diag} onClose={() => setSettings(false)} />;
  } else if (state.matches("idle")) {
    screen = <Home api={api} send={send} diag={diag} />;
  } else if (state.matches("inspecting")) {
    screen = (
      <section className="screen" aria-live="polite">
        <h1>Checking files…</h1>
        <p>Each file is opened in its own sandbox to read its pages.</p>
      </section>
    );
  } else if (state.matches("needsInput") || state.matches("rejected")) {
    screen = <Review api={api} ctx={ctx} send={send} rejected={state.matches("rejected")} />;
  } else if (state.matches("configuring") || state.matches("preflight")) {
    screen = <Configure api={api} ctx={ctx} send={send} registry={registry} />;
  } else if (state.matches("queued") || state.matches("processing") || state.matches("cancelling")) {
    screen = <ProgressScreen ctx={ctx} send={send} cancelling={state.matches("cancelling")} />;
  } else if (state.matches("preview")) {
    const sub = typeof state.value === "object" ? String(Object.values(state.value)[0]) : "ready";
    screen = <Result api={api} ctx={ctx} send={send} state={sub} />;
  } else if (state.matches("failed")) {
    screen = <Failed ctx={ctx} send={send} />;
  } else if (state.matches("cancelled")) {
    screen = <Cancelled send={send} />;
  } else {
    screen = <Saved ctx={ctx} send={send} />;
  }

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">Local File Conversion Suite</span>
        <span className="local">Processed on your device</span>
        <button type="button" className="link" onClick={() => setSettings((s) => !s)} aria-pressed={settings}>
          Settings
        </button>
      </header>
      <main>{screen}</main>
    </div>
  );
}
