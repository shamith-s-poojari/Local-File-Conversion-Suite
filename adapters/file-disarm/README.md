# file-disarm

"Make safe to open": a content disarm and reconstruction pass. It writes a copy of each file without what can run, call out or hide a payload when the file is opened. Each file keeps its format and is named `<title>-safe.<ext>`; a macro-enabled Office file (`.docm`, `.xlsm`, `.pptm`) comes out as the plain `.docx`, `.xlsx` or `.pptx`.

| Format | Removed |
| --- | --- |
| PDF | JavaScript (document, page, form and link actions), automatic actions on open, actions that launch programs, submit or import form data, open other files, play media or hide fields, attachments, embedded media, XFA. Forms are flattened when they had actions. |
| docx, xlsx, pptx | VBA macros, ActiveX controls, embedded objects and files, relationships that load from outside the file (templates, pictures, linked objects, workbook links), links to local files and programs, DDE and linking fields, hyperlink actions that run a program. |
| svg, html | Scripts, event handlers, `javascript:` and non-image `data:` URLs, frames, objects, `foreignObject`, redirects, `<base>`, and anything loaded from outside the file (pictures, styles, fonts). |

Plain web links (and `#fragment` and `data:image/` references) stay. The `removeLinks` parameter removes the web links too.

## Verification

After writing, `verify.py` opens every output in a fresh `python -I` process and looks for what is supposed to be gone. If anything is found, the output is deleted and the job fails with `E_INTERNAL` and reason `activeContentRemains`; a verifier that can't report fails the job with reason `verifierFailed`. Reports hold kinds and counts only, never document text.

## Limits

- Excel 4.0 macro sheets can't be removed safely, so a workbook that has them fails verification.
- Removing an external workbook link from an xlsx leaves its formulas showing their last values; removing an embedded chart workbook from a pptx leaves the chart's picture but it can't be edited.
- A signed PDF loses its signature (warning `SIGNATURE_INVALIDATED`). A PDF with an open password needs the password; the output has none.
- XML with a DOCTYPE is refused as `E_INPUT_CORRUPT`.

Engines: pikepdf, lxml.
