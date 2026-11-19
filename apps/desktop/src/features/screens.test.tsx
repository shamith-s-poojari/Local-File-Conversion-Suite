// Component tests, rendered to static markup (Testing Library isn't installed
// and no dependencies may be added). They check what a person or a screen
// reader would get: text, roles, labels and which buttons are disabled.
// Behaviour that needs effects or events is tested in the machine and helper
// tests instead.

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";
import protectSchema from "../../../../adapters/pdf-protect/params.schema.json";
import type { Api, Diagnostics, JsonSchema, Registry } from "../api";
import { context, diagnostics, noApi, view } from "../fixtures";
import type { Context, JobView } from "../machines/app";
import { CHECK_NAMES, ERRORS, REASONS, SELFTEST_NAMES, STARTUP_FAILED, installCommand } from "../text";
import { SchemaForm, missingRequired } from "./SchemaForm";
import { Configure, Failed, Home, Result, Review, Saved, Settings } from "./screens";
import { inputNames } from "./screens/Result";
import { ConfirmBar, ErrorBox, FormatTag, OpenSettingsContext, Warnings } from "./screens/shared";
import { ProgressScreen } from "./screens/Progress";
import { Checking } from "./screens/Checking";
import { selfTestState } from "./diagnostics";
import { App } from "../App";
import { actionError } from "./actionError";
import { FileRow } from "./screens/Review";

const send = () => {};
const html = renderToStaticMarkup;
/** Markup escapes apostrophes. */
const esc = (t: string) => t.replace(/'/g, "&#x27;");

const job = (extra: Partial<JobView> = {}): JobView => ({
  jobId: "j_1",
  state: "succeeded",
  stages: [],
  warnings: [],
  outputs: [{ path: "0.pdf", suggestedName: "merged.pdf", extension: "pdf", sizeBytes: 5000 }],
  ...extra,
});

const registry: Registry = {
  adapters: [
    { manifest: { id: "pdf-protect", kind: "operation", title: "Protect or unlock PDF" }, paramsSchema: protectSchema as JsonSchema },
    { manifest: { id: "pdf-merge", kind: "operation", title: "Merge PDFs" }, paramsSchema: {} },
  ],
};

/** The primary button whose label starts with `label`. */
function primary(markup: string, label: string): string {
  const buttons = markup.match(/<button[^>]*class="primary"[^>]*>[\s\S]*?<\/button>/g) ?? [];
  const found = buttons.find((b) => b.includes(label));
  if (!found) throw new Error(`no primary button "${label}" in ${buttons.join("\n")}`);
  return found;
}

describe("Result", () => {
  const render = (ctx: Partial<Context>, state = "ready") =>
    html(<Result api={noApi} ctx={context({ job: job(), ...ctx })} send={send} state={state} />);

  test("shows the outputs, their warnings and the save button", () => {
    const m = render({ job: job({ warnings: [{ code: "TAGS_DROPPED" }] }) });
    expect(m).toContain("merged.pdf");
    expect(m).toContain("Accessibility tags weren");
    expect(m).toContain("1 file is ready");
    expect(primary(m, "Save…")).not.toContain("disabled");
  });

  test("a partial job says so, and many outputs say Save all", () => {
    const outputs = [0, 1].map((i) => ({ path: `${i}.pdf`, suggestedName: `p${i}.pdf`, extension: "pdf", sizeBytes: 1 }));
    const m = render({ job: job({ state: "partial", outputs }) });
    expect(m).toContain("Partly done");
    expect(m).toContain("Save all…");
  });

  test("the save button is disabled and says so while saving", () => {
    const m = render({}, "saving");
    expect(primary(m, "Saving…")).toContain("disabled");
  });

  test("passed checks are named, not numbered (U24)", () => {
    const m = render({ job: job({ stages: ["verified-V1", "verified-V4", "verified-V9"] }) });
    expect(m).toContain(CHECK_NAMES.V1);
    expect(m).toContain(CHECK_NAMES.V4);
    expect(m).not.toMatch(/Passed these checks: V1/);
    // A check this app has no name for is still shown, not dropped.
    expect(m).toContain("V9");
    expect(m).toContain("not a guarantee");
  });

  test("files left out as already in the target format are counted (U22)", () => {
    const m = render({ to: "png", skipped: 2 });
    expect(m).toContain("Skipped 2 already in PNG");
    expect(render({ to: "png", skipped: 0 })).not.toContain("Skipped");
  });

  test("the heading takes focus and the outcome is announced (U11)", () => {
    const m = render({});
    expect(m).toMatch(/<h1[^>]*tabindex="-1"/);
    expect(m).toMatch(/role="status"/);
  });

  test("a save that needs confirming offers to replace the original", () => {
    const m = render({ save: { status: "confirmOverwrite", count: 0 } });
    expect(m).toContain("Replace the original");
  });
});

describe("Failed", () => {
  const apiWith = { getDiagnostics: async () => diagnostics } as unknown as Api;
  const render = (code: string, ctx: Partial<Context> = {}) =>
    html(<Failed api={apiWith} ctx={context({ job: job({ state: "failed", error: { code, retryable: false } }), ...ctx })} send={send} />);

  test("says what happened and what to do", () => {
    const m = render("E_TIMEOUT");
    expect(m).toContain(ERRORS.E_TIMEOUT!.what);
    expect(m).toContain("Change settings");
    expect(m).toContain("Start over");
  });

  test("is an alert and the heading takes focus (U11)", () => {
    const m = render("E_TIMEOUT");
    expect(m).toContain('role="alert"');
    expect(m).toMatch(/<h1[^>]*tabindex="-1"/);
  });

  test("offers Copy diagnostics where the text says to report it (U30)", () => {
    expect(render("E_INTERNAL")).toContain("Copy diagnostics");
    expect(render("E_PROTOCOL")).toContain("Copy diagnostics");
    expect(render("E_TIMEOUT")).not.toContain("Copy diagnostics");
  });

  test("a failed redaction offers the image fallback once", () => {
    expect(render("E_REDACTION_VERIFICATION_FAILED")).toContain("Use images for affected pages");
    expect(render("E_REDACTION_VERIFICATION_FAILED", { params: { rasterizeFallback: true } })).not.toContain(
      "Use images for affected pages",
    );
  });

  test("the limit text doesn't send people to Settings, which has no limits (U4)", () => {
    const m = render("E_LIMIT_EXCEEDED");
    expect(m).not.toContain("Settings");
    expect(m).not.toContain("Raise the limit");
  });

  test("an action that doesn't fit the job's state has text (E_WRONG_STATE)", () => {
    expect(ERRORS.E_WRONG_STATE).toBeDefined();
    expect(render("E_WRONG_STATE")).toContain(esc(ERRORS.E_WRONG_STATE!.what));
  });
});

describe("Settings", () => {
  const registryWithNames: Registry = {
    adapters: [
      { manifest: { id: "pdf-merge", kind: "operation", title: "Merge PDFs" }, paramsSchema: {} },
      { manifest: { id: "image-convert", kind: "conversion", title: "Convert pictures" }, paramsSchema: {} },
    ],
  };
  const render = () => html(<Settings diag={diagnostics} onClose={send} registry={registryWithNames} />);
  /** What's visible without opening a disclosure. */
  const visible = (m: string) => m.replace(/<details[\s\S]*?<\/details>/g, "");

  test("shows plain names for tools, engines and checks", () => {
    const m = visible(render());
    expect(m).toContain("Merge PDFs");
    expect(m).toContain("Convert pictures");
    expect(m).toContain("FFmpeg");
    expect(m).toContain(esc(SELFTEST_NAMES.read_sentinel!));
    expect(m).toContain(esc(SELFTEST_NAMES.connect_external!));
  });

  test("keeps engine, tool and self-test IDs behind a disclosure (U23)", () => {
    const m = render();
    const shown = visible(m);
    for (const id of ["pdf2docx", "read_sentinel", "connect_external", "image-convert", "pdf-merge"]) {
      expect(shown, id).not.toContain(id);
      expect(m, id).toContain(id);
    }
  });

  test("says which checks failed, and that processing is on", () => {
    const m = render();
    expect(m).toContain("Enabled");
    expect(m).toMatch(/Failed/);
  });

  test("offers Copy diagnostics and a way back", () => {
    const m = render();
    expect(m).toContain("Copy diagnostics");
    expect(m).toContain("Back");
  });

  test("renders while diagnostics are loading", () => {
    expect(html(<Settings diag={null} onClose={send} registry={null} />)).toContain("Loading");
  });

  test("a startup error is shown with its detail", () => {
    const m = html(<Settings diag={{ ...diagnostics, startupError: "the worker folder is missing" }} onClose={send} registry={null} />);
    expect(m).toContain("the worker folder is missing");
  });
});

describe("FileRow", () => {
  const row = (extra = {}, id = "a") => html(<FileRow api={noApi} f={view(id, extra)} send={send} />);

  test("shows the name, type and a remove button", () => {
    const m = row();
    expect(m).toContain("a.pdf");
    expect(m).toContain("PDF");
    expect(m).toContain('aria-label="Remove a.pdf"');
  });

  test("a file that needs a password asks for it, and Open waits for one", () => {
    const m = row({ needs: "password" });
    expect(m).toContain("Password to open this file");
    expect(m).toMatch(/<button[^>]*disabled=""[^>]*>Open<\/button>|<button[^>]*class="primary"[^>]*disabled=""[^>]*>Open/);
  });

  test("a file valid as several types asks which to use", () => {
    const m = row({ needs: "format", file: { ...view("a").file!, candidates: ["docx", "xlsx"] } });
    expect(m).toContain("Treat it as:");
    expect(m).toContain("Word");
    expect(m).toContain("Excel");
  });

  test("a cloud-only file offers to download it", () => {
    expect(row({ needs: "download" })).toContain("Download and open");
  });

  test("an unreadable file shows why, and has no remove button without an ID", () => {
    const m = html(<FileRow api={noApi} f={{ displayName: "x.bin", error: "unreadable" }} send={send} />);
    expect(m).toContain("The file couldn&#x27;t be read.");
    expect(m).not.toContain("Remove x.bin");
  });

  test("what the file reveals is listed behind a disclosure", () => {
    const m = row({ inspect: { pages: 2, encryption: "none", signed: false, warnings: [], findings: [{ kind: "location" }] } });
    expect(m).toContain("Reveals: location");
  });
});

describe("Review", () => {
  test("a rejected screen offers Back; the heading takes focus (U11)", () => {
    const m = html(<Review api={noApi} ctx={context({ files: [{ displayName: "x.bin", error: "unreadable" }] })} send={send} rejected />);
    expect(m).toContain("These files can&#x27;t be used");
    expect(m).toContain("Back");
    expect(m).toMatch(/<h1[^>]*tabindex="-1"/);
  });
});

describe("Configure: what's missing (U9)", () => {
  const render = (ctx: Partial<Context>, busy = false) =>
    html(
      <Configure
        api={noApi}
        ctx={context({ operation: "pdf-protect", params: { mode: "protect" }, ...ctx })}
        send={send}
        registry={registry}
        busy={busy}
      />,
    );

  test("says what's missing when nothing is chosen", () => {
    const m = render({ operation: null });
    expect(m).toContain("Needs: a format or a tool");
    expect(primary(m, "Start")).toContain("disabled");
  });

  test("a password is required to protect (U9, U15)", () => {
    const m = render({});
    expect(m).toContain("Needs: password to open");
    expect(primary(m, "Start")).toContain("disabled");
    expect(primary(m, "Start")).toContain("aria-describedby");
  });

  test("it must be typed twice, and the two must match (U15)", () => {
    const one = render({ params: { mode: "protect", userPassword: "abc" } });
    expect(one).toContain("Confirm password");
    expect(one).toContain("Needs:");
    expect(primary(one, "Start")).toContain("disabled");
    const wrong = render({ params: { mode: "protect", userPassword: "abc", userPasswordConfirm: "abd" } });
    expect(wrong).toContain("The passwords don&#x27;t match.");
    expect(primary(wrong, "Start")).toContain("disabled");
    const ok = render({ params: { mode: "protect", userPassword: "abc", userPasswordConfirm: "abc" } });
    expect(ok).not.toContain("Needs:");
    expect(primary(ok, "Start")).not.toContain("disabled");
  });

  test("unlocking needs no new password", () => {
    const m = render({ params: { mode: "unlock" } });
    expect(m).not.toContain("Needs:");
    expect(primary(m, "Start")).not.toContain("disabled");
  });

  test("shows busy and stops a second click while the job is being checked", () => {
    const m = render({ params: { mode: "unlock" } }, true);
    expect(m).toContain("Starting…");
    expect(primary(m, "Starting…")).toContain("disabled");
    expect(m).toContain('aria-busy="true"');
  });

  test("the heading takes focus (U11)", () => {
    expect(render({})).toMatch(/<h1[^>]*tabindex="-1"/);
  });
});

describe("SchemaForm field errors (U27)", () => {
  const form = (schema: JsonSchema, params: Record<string, unknown>, invalid: string[]) =>
    html(<SchemaForm schema={schema} params={params} onChange={send} ctx={{ files: [], pages: 0, reference: "", api: noApi }} invalid={invalid} />);

  test("a rejected number says the range, and its field is marked", () => {
    const schema: JsonSchema = { type: "object", properties: { quality: { type: "integer", title: "Quality", minimum: 10, maximum: 100 } } };
    const m = form(schema, { quality: 500 }, ["/quality"]);
    expect(m).toContain("Enter a number from 10 to 100.");
    expect(m).not.toContain("This value isn&#x27;t valid.");
    expect(m).toContain('aria-invalid="true"');
    expect(m).toContain('aria-describedby="param-quality-error"');
    expect(m).toContain('id="param-quality-error"');
  });

  test("a field that is fine isn't marked", () => {
    const schema: JsonSchema = { type: "object", properties: { quality: { type: "integer", title: "Quality" } } };
    const m = form(schema, { quality: 5 }, []);
    expect(m).not.toContain("aria-invalid");
  });

  test("text, choice and password fields each get their own message", () => {
    const schema: JsonSchema = {
      type: "object",
      properties: {
        text: { type: "string", title: "Text", maxLength: 40 },
        mode: { title: "Mode", enum: ["a", "b"] },
        userPassword: { type: "string", title: "Password to open", "x-ui": "password" },
      },
    };
    const m = form(schema, {}, ["/text", "/mode", "/userPassword"]);
    expect(m).toContain("Use at most 40 characters.");
    expect(m).toContain("Choose one of the options.");
    expect(m).toContain("Enter a password.");
    expect((m.match(/aria-invalid="true"/g) ?? []).length).toBeGreaterThanOrEqual(3);
  });

  test("a colour that isn't a hex colour shows its error, and blocks Start", () => {
    const schema: JsonSchema = { type: "object", properties: { color: { type: "string", title: "Colour", "x-ui": "color" } } };
    const m = form(schema, { color: "red" }, []);
    expect(m).toContain("Enter a colour like #1F6C9F.");
    expect(m).toContain('aria-invalid="true"');
    expect(missingRequired(schema, { color: "red" })).toEqual(["color"]);
    expect(missingRequired(schema, { color: "#1F6C9F" })).toEqual([]);
  });

  test("a page-range field keeps its own help and adds the error", () => {
    const schema: JsonSchema = { type: "object", properties: { pages: { type: "string", title: "Pages", "x-ui": "pageRanges" } } };
    const m = form(schema, { pages: "1-2" }, ["/pages"]);
    expect(m).toMatch(/aria-describedby="param-pages-help param-pages-error"/);
  });
});

describe("Home banner", () => {
  const render = (diag: Diagnostics | null) => html(<Home api={noApi} send={send} diag={diag} />);
  // Static markup escapes apostrophes.
  const startup = STARTUP_FAILED.what.replace(/'/g, "&#x27;");
  const sandboxOff = ERRORS.E_SANDBOX_UNAVAILABLE?.what ?? "";

  test("a core that couldn't start says the app is incomplete, not that the sandbox failed", () => {
    const m = render({ ...diagnostics, processingEnabled: false, startupError: "the resources folder has no worker runtime" });
    expect(m).toContain(startup);
    expect(m).not.toContain(sandboxOff);
    expect(m).toMatch(/<details>.*the resources folder has no worker runtime.*<\/details>/s);
  });

  test("a failed self-test still says processing is off", () => {
    const m = render({ ...diagnostics, processingEnabled: false });
    expect(sandboxOff).not.toBe("");
    expect(m).toContain(sandboxOff);
    expect(m).not.toContain(startup);
  });
});

describe("Home dropzone (U25)", () => {
  const render = (dragging: boolean) => html(<Home api={noApi} send={send} diag={null} dragging={dragging} />);

  test("is highlighted while files are dragged over the window", () => {
    expect(render(true)).toMatch(/class="dropzone[^"]*drag-over/);
    expect(render(false)).not.toContain("drag-over");
  });

  test("offers files and folders, and the heading takes focus (U11)", () => {
    const m = render(false);
    expect(m).toContain("Choose files");
    expect(m).toContain("Choose folder");
    expect(m).toMatch(/<h1[^>]*tabindex="-1"/);
  });
});

describe("ErrorBox: reason first (U1)", () => {
  test("a reason's text comes before its code's", () => {
    const m = html(<ErrorBox code="E_PARAMS_INVALID" reason="allSkipped" />);
    expect(m).toContain("Every file is already in that format.");
    expect(m).not.toContain(esc(ERRORS.E_PARAMS_INVALID!.what));
  });

  test("passwordsDiffer asks to convert the files separately", () => {
    expect(html(<ErrorBox code="E_PARAMS_INVALID" reason="passwordsDiffer" />)).toContain("separately");
  });

  test("selfTestPending says to try again in a moment", () => {
    const m = html(<ErrorBox code="E_SANDBOX_UNAVAILABLE" reason="selfTestPending" />);
    expect(m).toContain("Checking the sandbox");
    expect(m).toContain("in a moment");
  });

  test("a reason with no text of its own falls back to the code's", () => {
    expect(html(<ErrorBox code="E_LIMIT_EXCEEDED" reason="unknownThing" />)).toContain(esc(ERRORS.E_LIMIT_EXCEEDED!.what));
    expect(html(<ErrorBox code="E_LIMIT_EXCEEDED" />)).toContain(esc(ERRORS.E_LIMIT_EXCEEDED!.what));
  });

  test("an Office password says to remove it, and offers no password field", () => {
    const m = html(<ErrorBox code="E_PASSWORD_REQUIRED" reason="officePassword" />);
    expect(m).toContain("Remove the password");
    expect(m).not.toContain("<input");
  });

  test("engineMissing shows the install command as text to copy (U3)", () => {
    const diag = { ...diagnostics, engines: ["libreoffice"], platform: "macos" };
    const m = html(<ErrorBox code="E_INSTALL_CORRUPT" reason="engineMissing" diag={diag} />);
    expect(m).toContain("FFmpeg is missing.");
    expect(m).toContain(`<code class="install-cmd">${installCommand("ffmpeg", "macos")}</code>`);
    expect(m).not.toContain("LibreOffice is missing");
  });

  test("engineMissing without engine facts still says something", () => {
    const m = html(<ErrorBox code="E_INSTALL_CORRUPT" reason="engineMissing" />);
    expect(m).toContain(esc(REASONS.engineMissing!.what));
    expect(m).not.toContain("install-cmd");
  });

  test("actionError uses the reason's text too", () => {
    expect(actionError({ code: "E_INPUT_CORRUPT", reason: "downloadFailed" })).toBe(REASONS.downloadFailed!.what);
    expect(actionError({ code: "E_INPUT_CORRUPT" })).toBe(ERRORS.E_INPUT_CORRUPT!.what);
  });
});

describe("FileRow: reasons with why and what to do (U5)", () => {
  test("an Office file's password shows what, why and what to do", () => {
    const m = html(
      <FileRow api={noApi} f={view("w", { needs: undefined, error: "E_PASSWORD_REQUIRED", reason: "officePassword" })} send={send} />,
    );
    expect(m).toContain("This file is password protected.");
    expect(m).toContain(esc(REASONS.officePassword!.why!));
    expect(m).toContain("Remove the password");
    expect(m).not.toContain("Password to open this file");
  });
});

describe("Configure: every file skipped (E1)", () => {
  test("shows the allSkipped text, not the generic settings one", () => {
    const m = html(
      <Configure api={noApi} ctx={context({ error: { code: "E_PARAMS_INVALID", reason: "allSkipped" } })} send={send} registry={registry} />,
    );
    expect(m).toContain("Every file is already in that format.");
    expect(m).not.toContain("Some settings aren");
  });
});

describe("Saved (U7)", () => {
  const saved = (save: Context["save"]) => html(<Saved api={noApi} ctx={context({ save })} send={send} />);

  test("shows where the file went, as text, with a Show in folder button", () => {
    const m = saved({ status: "saved", count: 1, path: "/Users/me/Documents/a & b.pdf", revealToken: "tok" });
    expect(m).toContain("Saved to");
    expect(m).toContain("/Users/me/Documents/a &amp; b.pdf");
    expect(m).toContain("Show in folder");
    expect(m).not.toContain("<a ");
  });

  test("without a token there is no button, but the place is still shown", () => {
    const m = saved({ status: "saved", count: 2, path: "/Users/me/out", revealToken: null });
    expect(m).toContain("/Users/me/out");
    expect(m).not.toContain("Show in folder");
  });

  test("with neither, only the plain message", () => {
    const m = saved({ status: "saved", count: 1 });
    expect(m).not.toContain("Saved to");
    expect(m).not.toContain("Show in folder");
  });
});

describe("Result: Partly done lists what failed (U8)", () => {
  const files = ["a", "b", "c"].map((id) => view(id));
  const partial = (warnings: JobView["warnings"], extra: Partial<Context> = {}) =>
    html(
      <Result
        api={noApi}
        ctx={context({ files, job: job({ state: "partial", warnings }), ...extra })}
        send={send}
        state="ready"
      />,
    );

  // The names come from the core's staged order (`jobInputs`), fetched by
  // Result in an effect; static markup can't run it, so Warnings gets them.
  const named = (warnings: JobView["warnings"], staged: string[]) =>
    html(<Warnings warnings={warnings} prominent inputNames={inputNames(staged, files)} />);

  test("each failed part names its files and says why", () => {
    const m = named(
      [
        { code: "PART_FAILED", detail: { code: "E_INPUT_CORRUPT", inputs: [0, 2] } },
        { code: "PART_FAILED", detail: { code: "E_TIMEOUT", inputs: [1] } },
      ],
      ["a", "b", "c"],
    );
    expect(m).toContain("a.pdf, c.pdf");
    expect(m).toContain(esc(ERRORS.E_INPUT_CORRUPT!.what));
    expect(m).toContain("b.pdf");
    expect(m).toContain(esc(ERRORS.E_TIMEOUT!.what));
  });

  test("numbers index the core's staged order, whatever the selection or order param was", () => {
    const warn: JobView["warnings"] = [{ code: "PART_FAILED", detail: { code: "E_TIMEOUT", inputs: [0] } }];
    expect(named(warn, ["c", "a", "b"])).toContain("c.pdf");
    // A file left out as already in the target format simply isn't staged.
    expect(named(warn, ["b"])).toContain("b.pdf");
  });

  test("until the order is known, files read as File N", () => {
    const m = partial([{ code: "PART_FAILED", detail: { code: "E_TIMEOUT", inputs: [0] } }]);
    expect(m).toContain("File 1");
  });

  test("a held overwrite offers both Replace and Keep the original", () => {
    const m = partial([], { save: { status: "confirmOverwrite", count: 0, confirmToken: "t" } });
    expect(m).toContain("Replace the original");
    expect(m).toContain("Keep the original");
  });

  test("an input with no name is still shown, and a detail in another shape falls back", () => {
    const m = partial([
      { code: "PART_FAILED", detail: { code: "E_TIMEOUT", inputs: [7] } },
      { code: "PART_FAILED", detail: "oops" },
    ]);
    expect(m).toContain("File 8");
    expect(m).toContain("Some files couldn");
  });

  test("safe details are shown; document text never is", () => {
    const m = partial([
      { code: "NO_TEXT_LAYER", detail: { pages: [2, 5] } },
      { code: "GLYPH_UNSUPPORTED", detail: { characters: ["secret"] } },
    ]);
    expect(m).toContain("Pages: 2, 5.");
    expect(m).not.toContain("secret");
  });
});

describe("Home banner while the self-test runs (UX-5)", () => {
  const render = (diag: Diagnostics | null) => html(<Home api={noApi} send={send} diag={diag} />);
  const running: Diagnostics = { ...diagnostics, processingEnabled: false, sandbox: null };

  test("shows a neutral status, not the red alert", () => {
    const m = render(running);
    expect(m).toContain("Checking that the sandbox works");
    expect(m).toContain('role="status"');
    expect(m).not.toContain('role="alert"');
    expect(m).not.toContain("banner error");
    expect(m).not.toContain(esc(ERRORS.E_SANDBOX_UNAVAILABLE!.what));
  });

  test("the states are told apart", () => {
    expect(selfTestState(running)).toBe("running");
    expect(selfTestState({ ...diagnostics, processingEnabled: false })).toBe("failed");
    expect(selfTestState({ ...diagnostics, processingEnabled: true, sandbox: { passed: true, attempts: [] } })).toBe("passed");
    expect(selfTestState({ ...running, startupError: "x" })).toBe("startupFailed");
    expect(selfTestState(null)).toBe("unknown");
  });

  test("a failed test links to Settings when the app can open it (UX-6)", () => {
    const failed = { ...diagnostics, processingEnabled: false };
    expect(render(failed)).not.toContain("Open Settings");
    const m = html(
      <OpenSettingsContext.Provider value={() => {}}>
        <Home api={noApi} send={send} diag={failed} />
      </OpenSettingsContext.Provider>,
    );
    expect(m).toContain("Open Settings");
  });
});

describe("Settings link where it's needed (UX-6)", () => {
  const withSettings = (node: React.ReactNode) => html(<OpenSettingsContext.Provider value={() => {}}>{node}</OpenSettingsContext.Provider>);
  test("a missing engine's message links to Settings", () => {
    expect(withSettings(<ErrorBox code="E_INTERNAL" reason="engineMissing" diag={diagnostics} />)).toContain("Open Settings");
    expect(withSettings(<ErrorBox code="E_SANDBOX_UNAVAILABLE" />)).toContain("Open Settings");
    expect(withSettings(<ErrorBox code="E_TIMEOUT" />)).not.toContain("Open Settings");
  });
});

describe("leaving while it works (UX-7)", () => {
  test("the checking screen has a Cancel button and a labelled heading", () => {
    const m = html(<Checking send={send} />);
    expect(m).toContain("Cancel");
    expect(m).toMatch(/<h1[^>]*tabindex="-1"/);
  });

  test("setting up the job can be cancelled", () => {
    const m = html(<Configure api={noApi} ctx={context({ operation: "pdf-merge" })} send={send} registry={registry} busy />);
    expect(m).toMatch(/<button[^>]*>Cancel<\/button>/);
  });
});

describe("keeping focus on a waiting button (A11Y)", () => {
  test("Convert that isn't ready is aria-disabled, so it stays reachable", () => {
    const m = html(<Configure api={noApi} ctx={context({ operation: null })} send={send} registry={registry} />);
    expect(primary(m, "Start")).toContain('aria-disabled="true"');
    expect(primary(m, "Start")).not.toMatch(/\sdisabled/);
  });

  test("Cancel while cancelling is aria-disabled, not removed from the tab order", () => {
    const m = html(<ProgressScreen ctx={context({ job: job({ state: "running" }) })} send={send} cancelling />);
    expect(m).toMatch(/<button[^>]*aria-disabled="true"[^>]*>Cancel<\/button>/);
    expect(m).not.toMatch(/<button[^>]*\sdisabled/);
  });
});

describe("confirming before work is lost (UX-8)", () => {
  test("the question is a dialog with the safe answer first", () => {
    const m = html(<ConfirmBar message="Sure?" confirm="Yes" keep="No" onConfirm={() => {}} onKeep={() => {}} />);
    expect(m).toContain('role="alertdialog"');
    expect(m.indexOf(">No<")).toBeLessThan(m.indexOf(">Yes<"));
  });

  test("Result asks nothing until Adjust is pressed, and offers it as a button", () => {
    const m = html(<Result api={noApi} ctx={context({ job: job() })} send={send} state="ready" />);
    expect(m).not.toContain("alertdialog");
    expect(m).toContain(">Adjust<");
  });
});

describe("tool list failure can be retried (UX-10)", () => {
  test("shows what happened and a Try again button", () => {
    const m = html(<Configure api={noApi} ctx={context()} send={send} registry={null} registryFailed onRetryRegistry={() => {}} />);
    expect(m).toContain("The list of tools couldn");
    expect(m).toContain("Try again");
  });
  test("no message while it simply hasn't loaded yet", () => {
    const m = html(<Configure api={noApi} ctx={context()} send={send} registry={null} />);
    expect(m).not.toContain("couldn&#x27;t be loaded");
  });
});

describe("small markup fixes (A11Y)", () => {
  test("an unknown type is spoken, not read as a question mark", () => {
    expect(html(<FormatTag format="unknown" />)).toContain("Unknown type");
  });

  test("a field's description and error are both tied to it", () => {
    const schema = { type: "object", properties: { note: { type: "string", title: "Note", description: "Shown on the stamp." } } };
    const ctx = { files: [], pages: 0, reference: "", api: noApi };
    const m = html(<SchemaForm schema={schema} params={{ note: "x" }} onChange={() => {}} ctx={ctx} invalid={["/note"]} />);
    expect(m).toContain('aria-describedby="param-note-hint param-note-error"');
    expect(m).toContain('id="param-note-hint"');
  });

  test("a list field shows its items one per line and says so", () => {
    const schema = { type: "object", properties: { words: { type: "array", items: { type: "string" }, title: "Words" } } };
    const ctx = { files: [], pages: 0, reference: "", api: noApi };
    const m = html(<SchemaForm schema={schema} params={{ words: ["a", "b"] }} onChange={() => {}} ctx={ctx} invalid={[]} />);
    expect(m).toContain(">a\nb</textarea>");
    expect(m).toContain("One per line.");
  });

  test("a required field is marked for assistive technology", () => {
    const schema = { type: "object", required: ["name"], properties: { name: { type: "string", title: "Name" } } };
    const ctx = { files: [], pages: 0, reference: "", api: noApi };
    expect(html(<SchemaForm schema={schema} params={{}} onChange={() => {}} ctx={ctx} invalid={[]} />)).toContain('aria-required="true"');
  });

  test("the page's main area can take the skip link's focus", () => {
    expect(html(<App api={noApi} />)).toContain('<main id="main" tabindex="-1">');
  });
});
