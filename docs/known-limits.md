# Known limits

What the app can't do, or does only partly, as of Phase 1. Each tool's README has the details; this page collects them for users and reviewers.

## Platforms

| | macOS | Linux | Windows |
|---|---|---|---|
| Sandbox | Works (Seatbelt) | Written, not yet verified | **Not implemented: processing is off** |
| Installer | Not built yet | Not built yet | Not built yet |

Packaging, signing and notarization (T21) are not done, so the app only runs from a development checkout.

## Files

- **Only PDF files can be processed** in this version. Other formats are recognised and explained, but no tool accepts them yet.
- **Limits:** 5,000 pages per file and 200 files per selection.
- **One open password per job.** Files with different open passwords must be unlocked one at a time first.
- **Online-only files** (OneDrive, iCloud Drive and similar) are downloaded only after you agree.
- **Repaired files:** a damaged PDF that can be repaired on opening may still be refused if the damage survives the rewrite (`E_INPUT_CORRUPT`).

## Tools

**Merge**
- Accessibility tags aren't merged (`TAGS_DROPPED`).
- XMP metadata isn't carried over; only the basic document properties are.
- Form fields whose names clash with an earlier file's are flattened (`FORMS_FLATTENED`).
- Per-file bookmarks use the original file names.

**Split**
- Form fields are always flattened, because a field can't be split across files.
- "Maximum size" is best effort: a single page larger than the limit gets a part of its own.

**Organize**
- If any page is deleted or duplicated, form fields are flattened.

**Protect or unlock**
- Permissions such as "no printing" are honored voluntarily by PDF viewers. They are not access control.
- Lost passwords can't be recovered. The app never guesses or cracks passwords.

**Watermark**
- Text only; image watermarks aren't available yet.
- Until the bundled font ships with packaging, only Western European characters can be shown. Others appear as "?" (`GLYPH_UNSUPPORTED`).
- A watermark is a label. It can be removed by anyone with a PDF editor.

**Redact**
- Search can't find text inside scanned images. Pages with images and no text raise `NO_TEXT_LAYER`. Drawing boxes still works on scans.
- The size of a box can hint at the length of what it hides.
- Embedded font subsets can reveal which characters a document used, though not where.
- Content that is meaningful elsewhere, such as a name inside a chart image, isn't detected.
- Redaction also removes all comments, form fields, attachments, hidden layers, document properties, and bookmarks that contain a term.
- Verification is reported as **"passed these checks"**, not a guarantee. If checks fail and you accept the fallback, affected pages become images and their text is lost.
- Drawing boxes needs a pointer. Terms and whole-page redaction work by keyboard.

## Interface

- Previews are images rendered in the sandbox. Text in the preview can't be selected or searched.
- The page organizer loads thumbnails lazily but lists every page. Very long documents (thousands of pages) are slow to scroll.
- The redaction canvas's match list covers the first 500 pages; terms are still applied to every page.

## Error codes

The catalog has no code for an invalid parameter or a precondition, such as "the file is already encrypted". These come back as `E_INTERNAL` with the parameter name and reason, and the interface shows them next to the field. A catalog code is planned.

## Privacy and security

- Deleting a job's files is ordinary deletion, not secure erasure. See [Privacy](privacy.md).
- WebView network traffic isn't measured yet (spike S4).
- Sync-folder detection is best effort.
- The packet-capture network test (`tools/network-test/capture.sh`) hasn't run yet; the canary test has, on macOS.
