import type { Api, Diagnostics } from "../../api";
import { FAMILIES, type Family } from "../../formats";
import { ArrowRight, ShieldIcon, ToolIcon, UploadIcon, WarningIcon } from "../../icons";
import { SELF_TEST_RUNNING, STARTUP_FAILED, errorMessage, installHint } from "../../text";
import { missingEngines, selfTestState } from "../diagnostics";
import { usePicker } from "../pickers";
import { FamilyGlyph, SettingsLink, type Send } from "./shared";

const FAMILY_TOOLS: Partial<Record<Family, string[]>> = {
  document: ["Make safe to open", "Remove hidden details"],
  image: ["Rotate and resize", "Compress", "Animated GIF", "Images to one PDF", "Contact sheet", "Remove hidden details"],
  audio: ["Join", "Volume, fade and speed", "Remove hidden details"],
  video: ["Join", "Rotate and speed", "Save frames or a contact sheet", "Remove hidden details"],
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
  "Make safe to open",
  "Contact sheet",
];

// ------------------------------------------------------------------- home

export function Home({
  api,
  send,
  diag,
  dragging = false,
}: {
  api: Api;
  send: Send;
  diag: Diagnostics | null;
  /** Files are being dragged over the window (audit U25). */
  dragging?: boolean;
}) {
  const { busy, error, choose } = usePicker(api, send);
  const disabled = diag !== null && !diag.processingEnabled;
  // Still running is not failed: no error colour, no alert (audit UX-5).
  const checking = selfTestState(diag) === "running";
  const missing = missingEngines(diag);
  return (
    <section className="screen home" aria-labelledby="home-title">
      <header className="hero reveal">
        <p className="eyebrow">Local file converter</p>
        <h1 id="home-title" className="display" tabIndex={-1}>
          Convert any file, right on this computer.
        </h1>
        <p className="lede">
          Documents, images, audio and video. Each file opens in its own sandbox with no network access, and nothing is
          uploaded anywhere.
        </p>
      </header>
      {checking && (
        <div className="banner" role="status">
          <div>
            <strong>{SELF_TEST_RUNNING.what}</strong>
            <p>{SELF_TEST_RUNNING.why}</p>
          </div>
        </div>
      )}
      {disabled && !checking && (
        <div className="banner error" role="alert">
          <WarningIcon />
          {diag?.startupError ? (
            <div>
              <strong>{STARTUP_FAILED.what}</strong>
              <p>{STARTUP_FAILED.todo}</p>
              <details>
                <summary>{STARTUP_FAILED.details}</summary>
                <p>{diag.startupError}</p>
              </details>
              <SettingsLink />
            </div>
          ) : (
            <div>
              <strong>{errorMessage("E_SANDBOX_UNAVAILABLE").what}</strong>
              <p>{errorMessage("E_SANDBOX_UNAVAILABLE").why}</p>
              <p>{errorMessage("E_SANDBOX_UNAVAILABLE").todo}</p>
              <SettingsLink />
            </div>
          )}
        </div>
      )}
      <div className="bezel reveal">
        <div className={`dropzone${dragging ? " drag-over" : ""}`}>
          <span className="drop-icon">
            <UploadIcon />
          </span>
          <p className="drop-title">Drop files or folders anywhere in this window</p>
          <p id="drop-help" className="hint">
            Up to 200 at once. Mixed types are fine. You can also paste a picture or a table.
          </p>
          <div className="drop-actions">
            <button type="button" className="primary lg" onClick={() => choose("files")} disabled={busy || disabled} aria-describedby="drop-help">
              <span>{busy ? "Opening…" : "Choose files"}</span>
              <span className="btn-icon">
                <ArrowRight />
              </span>
            </button>
            <button type="button" className="lg" onClick={() => choose("folder")} disabled={busy || disabled}>
              Choose folder
            </button>
          </div>
          {error && (
            <p className="hint error" role="alert">
              {error}
            </p>
          )}
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


