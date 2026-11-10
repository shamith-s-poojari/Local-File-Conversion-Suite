# pdf-redact

Removes the content under boxes the user draws and every match of search terms, then strips document-level content. A separate verifier proves the result before it is offered ([spec 13.8](../../docs/specs/phase-1.md#138-pdf-redact-pymupdf--pypdfium2--pikepdf)).

| | |
|---|---|
| **Tier** | E for pages that keep their text; rasterized pages (fallback only) lose their text |
| **Engines** | PyMuPDF (AGPL), pypdfium2 (Apache-2.0 / BSD), pikepdf (MPL-2.0) |
| **Widget** | `redactionCanvas`, fed by this adapter's `page_text` (word boxes in displayed orientation) |
| **Pool** | `redact`, one slot |

**A false "redacted" is much worse than a failed job.** If verification fails, the output is deleted and the job fails with `E_REDACTION_VERIFICATION_FAILED`. Nothing partial is ever offered.

## Pipeline

1. **Pre-pass (pikepdf):** decrypt, and remove content in optional-content layers that are off by default. This cuts `/OC … BDC … EMC` sections and hidden XObjects, in pages and in nested forms. MuPDF doesn't extract hidden-layer text, so it could never be found by search otherwise.
2. **Plan (PyMuPDF):**
   - Boxes arrive normalized in *displayed* orientation (after /Rotate and the CropBox).
   - Terms are matched case-insensitively. Matching ignores line breaks, hyphenation and spacing, so text split across operators or lines still matches. Invisible text (render mode 3) is included.
   - Text under each box is captured for V9.
3. **Redact (PyMuPDF):** removes text, blanks image pixels and drops covered line art under every region.
4. **Strip:**
   - Removed: metadata and XMP (document and per object), attachments, all annotations and form fields, JavaScript and actions, thumbnails and `/PieceInfo`.
   - Bookmarks whose title contains a term are removed.
   - Info keeps only a neutral Producer.
   - Removing form fields and annotations must be stated to the user *before* running (the `FORMS_FLATTENED` warning is raised).
5. **Fill and rewrite (pikepdf):** fill rectangles are drawn in PDF space from the CropBox and /Rotate, then the file is fully rewritten. The rewrite drops unreferenced objects and leaves one revision.
6. **Verify (`verify.py`, fresh process):** checks V1 to V9 on the output file alone.

## Verification

| ID | Check | Engine |
|---|---|---|
| V1 | No character inside any region | PyMuPDF |
| V2 | Same, with its **own** box-to-PDF-space arithmetic | pdfium |
| V3 | No term in the full text, whitespace and hyphenation normalized | both |
| V4 | No term in any decoded stream or string (Latin-1, UTF-8, UTF-16BE, raw or hex) | pikepdf |
| V5 | Only Producer in Info; no XMP, attachments, annotations, forms, JavaScript, thumbnails or hidden layers | pikepdf |
| V6 | Every region renders entirely in the fill color at 150 DPI | PyMuPDF |
| V7 | One `startxref`, one `%%EOF`, no `/Prev` | raw bytes |
| V8 | Page count unchanged | both |
| V9 | Text captured under boxes found elsewhere: **warning** `REDACTED_TEXT_REMAINS_ELSEWHERE` with count and pages | PyMuPDF |

Passed checks are reported as stages named `verified-V1` … `verified-V8`. **The UI should say "passed these checks", never "guaranteed".**

A verifier that crashes or prints anything other than a report counts as a failure (`V0`).

## Rasterize fallback

With `rasterizeFallback: true` (set only after the user accepts the offer), failed pages become 200 DPI images of the redacted rendering, and verification runs again. A document-level failure, such as V4 or V5, rasterizes every page. Text on those pages is lost, and the UI must say so.

## Why fills aren't drawn by MuPDF

On rotated pages whose CropBox origin isn't (0, 0), MuPDF removes the right content but draws new graphics, including redaction fills, offset by the crop origin. V6 caught this during development. So MuPDF only removes content, and this adapter paints the fills itself. A test checks that MuPDF's *pixel blanking* still lands correctly on such pages.

## Known limits (publish in the docs)

- Search can't find text inside scanned images. `NO_TEXT_LAYER` warns when a page has images and no text. Box redaction still works on scans.
- A box's size can hint at the length of the hidden content.
- Embedded font subsets may reveal which characters the document used, though not where.
- Content that is meaningful elsewhere, such as a name inside a chart image, isn't detected.
- Optional-content membership dictionaries (OCMDs) that involve any hidden group are treated as hidden and removed. This errs toward removing more.
- The verification report reaches the core only as stage names. A structured report message is a protocol follow-up.
