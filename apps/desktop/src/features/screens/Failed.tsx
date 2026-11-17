import type { Api } from "../../api";
import type { Context } from "../../machines/app";
import { WarningIcon } from "../../icons";
import { errorMessage } from "../../text";
import { CopyDiagnostics, Warnings, type Send } from "./shared";

export function Failed({ api, ctx, send }: { api: Api; ctx: Context; send: Send }) {
  const code = ctx.job?.error?.code ?? "E_INTERNAL";
  const m = errorMessage(code);
  const fallback = code === "E_REDACTION_VERIFICATION_FAILED" && ctx.params.rasterizeFallback !== true;
  return (
    <section className="screen narrow state-screen" aria-labelledby="failed-title">
      <span className="state-icon error">
        <WarningIcon />
      </span>
      {/* An alert, so a screen reader says what went wrong without a search for it. */}
      <div role="alert">
        <h1 id="failed-title" tabIndex={-1}>
          {m.what}
        </h1>
        {m.why && <p className="lede">{m.why}</p>}
        {m.todo && <p>{m.todo}</p>}
      </div>
      {m.report && <CopyDiagnostics load={() => api.getDiagnostics()} />}
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
