# ADR 0001: Scope of the requested 36-tool feature list

| | |
|---|---|
| **Status** | Proposed |
| **Date** | 2026-11-08 |
| **Deciders** | Team (all stream owners) |
| **Affects** | [Blueprint](../architecture/blueprint.md) sections 3.2, 8 and 10; [Phase 1 spec](../specs/phase-1.md) section 1 |

## Context

A list of 36 PDF and conversion tools was proposed as the product's feature set. The blueprint's scope is locked (blueprint section 10), and the Phase 1 spec is approved. This ADR maps each requested tool onto the current plan and decides what happens to the tools the plan doesn't cover.

The constraints that matter most:

- **No network calls at all** (locked scope). Every tool must run fully offline.
- **12 formats** in the sniffer and matrix: `pdf docx pptx xlsx md html txt png jpg webp svg heic`. Each new format means sniffer cases, matrix cells, routing invariants, an engine, a license review and corpus files.
- **4 to 5 people, about 20 weeks**, with a week 6 checkpoint that may already force cuts.
- **Installer size** is estimated at 700 MB or more for the Full edition.
- **Security model**: every parser of untrusted input runs in a sandboxed worker (blueprint section 6, spec C2).

## Mapping

### Already planned (no change needed)

| Requested tool | Where it lives | Phase | Notes |
|---|---|---|---|
| Merge PDF | `pdf-merge` | 1 | |
| Split PDF | `pdf-split` | 1 | |
| Delete Pages | `pdf-organize` | 1 | Omitted pages are deleted |
| Rotate Pages | `pdf-organize` | 1 | 90/180/270, relative to existing rotation |
| Add Watermark | `pdf-watermark` | 1 | Text and image (image is *cuttable*) |
| Protect PDF | `pdf-protect` (mode `protect`) | 1 | AES-256 |
| Unlock PDF | `pdf-protect` (mode `unlock`) | 1 | Requires the correct password. No cracking or guessing, per spec 13.6 |
| PDF to JPG / PNG / WebP / SVG | `pdf-to-image` | 2 | Tiers L / H / L / H |
| PDF to Plain Text | `pdf-to-text` | 2 | Tier R; exact when a text layer exists |
| Word / Excel / PowerPoint to PDF | `office-to-pdf` | 2 | Tier H |
| Image to PDF | `image-to-pdf` | 2 | PNG, JPG, WebP, HEIC, SVG only. Not TIFF, BMP or GIF |
| Compress PDF | `pdf-compress` | 3 | Two stages; keeps the original if the output is larger |
| Extract Text (OCR) | `pdf-ocr` | 3 | English bundled, other languages optional |
| PDF to Word | pdf2docx | 3 | Tier R, behind "Show best-effort" |
| PDF to Excel | pdfplumber + openpyxl | 3 | Tier R, tables only, behind "Show best-effort" |

### Partly covered

| Requested tool | Current plan | Gap |
|---|---|---|
| View / Read PDF | Sandbox-rendered PNG page previews (spec C2) | No full reader: no text selection, search or zoom-to-vector. PDF.js was removed for security reasons (C2) |
| PDF to HTML | Matrix cell PDF → HTML is tier R | No phase schedules an adapter for it |

### Explicitly out of v1 already

| Requested tool | Reason |
|---|---|
| PDF to PowerPoint | Deferred in the locked scope (blueprint section 10) |

### Not in the plan

| Requested tool | Likely engine | Cost and risk |
|---|---|---|
| Edit PDF (change existing text, layout) | None suitable | Very high. Editing positioned glyphs needs font subsetting, reflow and custom UI. A product of its own |
| Edit PDF (add text, annotate) | pikepdf + ReportLab, as in watermark | Medium. Needs a custom `x-ui` widget for placement |
| Sign PDF (drawn or image signature) | pikepdf + ReportLab overlay | Low to medium. Close to an image watermark placed on one page |
| Sign PDF (cryptographic digital signature) | pyHanko | High. Certificates, key storage, trust and timestamping. Timestamping needs the network |
| Bank Statement Converter | pdfplumber + per-bank heuristics | High. Every bank's layout differs, and wrong numbers do real harm. Financial data raises the stakes on accuracy |
| PDF to TIFF | PyMuPDF + Pillow | Low. One more output of `pdf-to-image`. New sniffer format only if TIFF also becomes an input |
| PDF to EPS | Ghostscript (`eps2write`) | Low to medium. Ghostscript is already accepted under AGPL |
| PDF to EPUB | PyMuPDF heuristics → Pandoc | Medium to high. Tier R; reflow from fixed layout is reconstruction |
| PDF to MOBI | Calibre (GPL) | Not worth it. Amazon no longer accepts MOBI; AZW3 or EPUB cover the use |
| PDF to AZW3 | Calibre (GPL, separate process) | High. Calibre is large and would grow the installer substantially |
| PDF to DXF | PyMuPDF vector paths → ezdxf | Medium. Niche audience; doesn't match the stated audience of students and office users |
| EPUB to PDF | Pandoc → WeasyPrint | Medium. EPUB is a ZIP of XHTML, so it needs the full XXE, zip-bomb and script suites |
| AI PDF Summarizer | Local LLM | **Conflicts with the no-network rule** unless the model ships in the installer |
| AI Document Chat | Local LLM + embeddings | Same conflict, plus a chat UI and retrieval index |
| Translate PDF | Local translation model + layout rewrite | Same conflict. Preserving layout after translation is also a hard fidelity problem |

## Decision

1. **The v1 roadmap does not change.** The 15 planned tools and the two partly covered ones stay where they are. Nothing in this list justifies adding work before the week 6 checkpoint.
2. **The unplanned tools become a v2 backlog**, grouped as below. Each group needs its own ADR before work starts, covering engine, license, sandbox, sniffer and matrix changes, and installer size.
   - **Cheap extensions** (fit the existing architecture): PDF to TIFF, add-text and annotate, drawn or image signature. These may be pulled into Phase 3 or 4 only if the week 6 checkpoint passes on all three OSes with no cuts.
   - **New output formats**: PDF to EPS, PDF to EPUB, EPUB to PDF, PDF to DXF, PDF to AZW3.
   - **Larger products**: full text editing, cryptographic signatures, Bank Statement Converter.
3. **PDF to MOBI is rejected.** The format is obsolete; EPUB and AZW3 cover the need.
4. **AI tools (Summarizer, Document Chat, Translate) are rejected for v1** and may return only under these conditions:
   - The model runs locally and makes no network calls, consistent with the locked scope and the network test (spec C4).
   - It ships as an optional, separately downloaded pack, so the Full and Lite installers don't grow.
   - Model inference runs in a sandboxed worker like every other engine, and the model's license is reviewed against AGPL distribution.
   - Output is labelled as machine-generated and possibly wrong.

## Consequences

- Marketing and README copy must list only the planned tools. The 36-tool list can't be presented as the v1 feature set.
- The sniffer stays at 12 formats in v1. Adding TIFF, EPUB or DXF as inputs later means new sniffer test cases, matrix cells and routing invariants.
- "Unlock PDF" keeps the spec 13.6 behavior: the user must know the password, and there is no password-removal or cracking feature.
- "View / Read PDF" stays a preview, not a reader, until a sandboxed full reader is designed. That would need its own ADR because of C2.
- Calibre, pyHanko, ezdxf and any AI model are **not approved dependencies**. The license checks in CI must keep failing on them until an ADR approves them.

## Open questions

- Should PDF → HTML get an adapter in Phase 3 alongside the other tier R routes, or be dropped from the matrix?
- Is a full in-app reader wanted badly enough to revisit C2?
- Which v2 group comes first, if the team ships v1 on schedule?
