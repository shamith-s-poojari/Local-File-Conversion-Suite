import { useState } from "react";
import type { Api } from "../../api";
import { familyOf, formatInfo } from "../../formats";
import { CloseIcon, LockIcon } from "../../icons";
import type { Context, FileView } from "../../machines/app";
import { type Message, warningText } from "../../text";
import { Password } from "../../widgets/Password";
import { actionMessage } from "../actionError";
import { ErrorBox, FamilyGlyph, FileProblem, FormatTag, fileMeta, Reveals, type Send } from "./shared";

// ----------------------------------------------------------------- review

export function Review({ api, ctx, send, rejected }: { api: Api; ctx: Context; send: Send; rejected: boolean }) {
  return (
    <section className="screen narrow" aria-labelledby="review-title">
      <header className="screen-head reveal">
        <p className="eyebrow">{rejected ? "Nothing to convert" : "One more thing"}</p>
        <h1 id="review-title" tabIndex={-1}>{rejected ? "These files can't be used" : "Check your files"}</h1>
      </header>
      {ctx.error && <ErrorBox code={ctx.error.code} reason={ctx.error.reason} />}
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

export function FileRow({ api, f, send }: { api: Api; f: FileView; send: Send }) {
  const [password, setPassword] = useState<string | undefined>();
  const [problem, setProblem] = useState<Message | null>(null);
  const id = f.file?.fileId;
  const act = async (fn: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await fn();
      send({ type: "RESOLVED" });
    } catch (e) {
      setProblem(actionMessage(e));
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
      <FileProblem f={f} />
      <Reveals f={f} />
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
            invalid={problem !== null}
            errorId={`pw-${id}-problem`}
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
        <div role="alert" id={`pw-${id}-problem`}>
          <p className="hint error">{problem.what}</p>
          {problem.why && <p className="hint">{problem.why}</p>}
          {problem.todo && <p className="hint">{problem.todo}</p>}
        </div>
      )}
    </li>
  );
}


