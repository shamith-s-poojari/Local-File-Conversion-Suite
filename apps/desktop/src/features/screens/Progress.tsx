import { useEffect, useState } from "react";
import { formatInfo } from "../../formats";
import type { Context } from "../../machines/app";
import { CheckIcon } from "../../icons";
import { checkName } from "../../text";
import { Warnings, type Send } from "./shared";

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
        <h1 id="progress-title" tabIndex={-1}>
          {cancelling ? "Cancelling…" : waiting ? "Waiting to start…" : "Working on it…"}
        </h1>
      </header>
      {/* What a screen reader hears: the stage, and progress in 10% steps. The
          visible numbers and the timer change every second and aren't live. */}
      <p className="visually-hidden" role="status">
        {progressAnnouncement(job?.stage, pct)}
      </p>
      <div className="bezel">
        <div className="progress-card">
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
        {/* aria-disabled keeps focus here once it's pressed. */}
        <button type="button" onClick={() => !cancelling && send({ type: "CANCEL" })} aria-disabled={cancelling || undefined}>
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

/** The sentence for a screen reader. It changes only when the stage does or
 * progress passes the next 10%, so it isn't read out on every update. */
export function progressAnnouncement(stage: string | undefined, pct: number | null): string {
  const label = stage ? stageLabel(stage) : "Preparing";
  return pct === null ? label : `${label}, ${Math.floor(pct / 10) * 10}%`;
}

export function stageLabel(stage: string): string {
  if (stage.startsWith("verified-")) return `Passed: ${checkName(stage.slice(9))}`;
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
