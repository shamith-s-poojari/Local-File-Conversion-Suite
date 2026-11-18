import type { Diagnostics, Registry } from "../../api";
import { GearIcon } from "../../icons";
import { selfTestName } from "../../text";
import { CopyDiagnostics, ErrorBox } from "./shared";

/** Engines the app looks for: the ID Rust reports, what to call it, and what it's for. */
const ENGINE_NAMES: [string, string, string][] = [
  ["ffmpeg", "FFmpeg", "Audio and video"],
  ["libreoffice", "LibreOffice", "Word, PowerPoint, Excel and OpenDocument"],
  ["pdf2docx", "PDF to Word converter", "PDF to Word"],
  ["pillow-heif", "HEIC photo reader", "iPhone HEIC photos"],
  ["markdown-it-py", "Markdown reader", "Markdown"],
  ["openpyxl", "Excel reader and writer", "Excel to and from CSV, JSON and YAML"],
  ["pyyaml", "YAML reader", "YAML files"],
];

export function Settings({ diag, onClose, registry }: { diag: Diagnostics | null; onClose: () => void; registry: Registry | null }) {
  const titles = new Map((registry?.adapters ?? []).map((a) => [a.manifest.id, a.manifest.title]));
  const tools = (diag?.adapters ?? []).flatMap((id) => (titles.has(id) ? [titles.get(id)!] : []));
  return (
    <section className="screen narrow" aria-labelledby="settings-title">
      <header className="screen-head reveal">
        <p className="eyebrow">
          <GearIcon /> Settings
        </p>
        <h1 id="settings-title" tabIndex={-1}>
          Settings and diagnostics
        </h1>
      </header>
      {!diag ? (
        <p role="status">Loading…</p>
      ) : (
        <>
          {diag.startupError && <ErrorBox code="E_INSTALL_CORRUPT" detail={diag.startupError} />}
          <dl className="facts card">
            <dt>Version</dt>
            <dd className="mono">{diag.appVersion ?? "unknown"}</dd>
            <dt>Processing</dt>
            <dd>{diag.processingEnabled ? "Enabled" : "Disabled"}</dd>
            <dt>Tools</dt>
            <dd className="small">{tools.length > 0 ? tools.join(", ") : "Loading…"}</dd>
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
            <>
              <p>{diag.sandbox.passed ? "Passed: the sandbox keeps files and the network out of reach." : "Failed: processing is turned off until this passes."}</p>
              <ul className="selftest-list card">
                {diag.sandbox.attempts.map((a) => (
                  <li key={a.name} className={a.passed ? "" : "fail"}>
                    <span className={`dot ${a.passed ? "on" : "off"}`} aria-hidden="true" />
                    <span>{selfTestName(a.name)}</span>
                    <span className="engine-state">{a.passed ? "Passed" : "Failed"}</span>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p role="status">Running…</p>
          )}
          <details className="technical">
            <summary>Technical details</summary>
            <dl className="facts">
              <dt>Worker protocol</dt>
              <dd className="mono">{diag.protocolVersion ?? "unknown"}</dd>
              <dt>Tool IDs</dt>
              <dd className="mono small">{(diag.adapters ?? []).join(", ")}</dd>
              <dt>Engine IDs found</dt>
              <dd className="mono small">{(diag.engines ?? []).join(", ") || "none"}</dd>
            </dl>
            {diag.sandbox && (
              <table className="selftest">
                <thead>
                  <tr>
                    <th>Check ID</th>
                    <th>Expected</th>
                    <th>Result</th>
                  </tr>
                </thead>
                <tbody>
                  {diag.sandbox.attempts.map((a) => (
                    <tr key={a.name} className={a.passed ? "" : "fail"}>
                      <td className="mono">{a.name}</td>
                      <td className="mono">{a.expected}</td>
                      <td className="mono">{a.outcome}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </details>
          <CopyDiagnostics load={async () => diag} />
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
