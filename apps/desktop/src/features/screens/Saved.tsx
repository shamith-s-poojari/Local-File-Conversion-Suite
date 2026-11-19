import { useState } from "react";
import type { Api } from "../../api";
import { ArrowRight, CheckIcon } from "../../icons";
import type { Context } from "../../machines/app";
import { SAVED } from "../../text";
import { actionError } from "../actionError";
import type { Send } from "./shared";

export function Saved({ api, ctx, send }: { api: Api; ctx: Context; send: Send }) {
  const n = ctx.save?.count ?? 1;
  const path = ctx.save?.path;
  const token = ctx.save?.revealToken;
  const [problem, setProblem] = useState<string | null>(null);
  // The token works once (audit U7), so after one try the button goes: a
  // second click could only say "nothing pending".
  const [tried, setTried] = useState(false);
  return (
    <section className="screen narrow state-screen" aria-labelledby="saved-title">
      <span className="state-icon ok">
        <CheckIcon />
      </span>
      <h1 id="saved-title" tabIndex={-1}>{n === 1 ? "Saved" : `Saved ${n} files`}</h1>
      {/* Plain text: the app never opens a path itself. Rust does, from the token. */}
      {path && (
        <p className="saved-path">
          {SAVED.where} <span className="mono">{path}</span>
        </p>
      )}
      {token && !tried && (
        <div className="actions center">
          <button
            type="button"
            onClick={() => {
              setProblem(null);
              setTried(true);
              Promise.resolve()
                .then(() => api.revealSaved(token))
                .catch((e) => setProblem(actionError(e)));
            }}
          >
            {SAVED.show}
          </button>
        </div>
      )}
      {problem && (
        <p className="hint error" role="alert">
          {problem}
        </p>
      )}
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
