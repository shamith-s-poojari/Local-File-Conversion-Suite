# office-convert

Converts office documents with LibreOffice, headless, inside the job's sandbox.

| From | To |
|---|---|
| Word (DOCX, DOC), ODT, RTF | PDF, Word, ODT, RTF, plain text, HTML, Markdown, EPUB, page images |
| PowerPoint (PPTX, PPT), ODP | PDF, PowerPoint, ODP, plain text, slide images |
| Excel (XLSX, XLS), ODS | PDF, Excel, ODS, CSV (one file per sheet), HTML, page images |
| CSV | Excel, ODS, PDF |
| Plain text | PDF, Word, ODT, RTF, HTML, Markdown |
| HTML | PDF, Word, ODT, Markdown, plain text |
| Markdown | Word, ODT, RTF, EPUB |

Images and slide text go through PDF inside the job (rendered with pdfium). Markdown is rendered with markdown-it first; HTML to Markdown is this SDK's own converter (`converter_sdk.markup`).

LibreOffice runs with a fresh profile in `work/`, never the user's. Macros don't run. Its single-instance pipe is the only thing it may create outside the job directory (`/tmp/OSL_PIPE_…`), and it may only listen on it, never connect to another instance (see `crates/app-core/src/engines.rs`).

Why inputs aren't disarmed first (audit P15): `file-disarm` removes embedded objects, linked content and similar parts. A conversion should keep what the document shows, so disarming first would silently change the result. The risks are limited another way: LibreOffice runs headless with its default macro security (it never asks to enable macros), the sandbox denies it the network (ADR 0003), and every job uses a fresh profile in `work/`. The output is a new file written by LibreOffice's export filters, and is re-sniffed before it can be saved. Users who want a document made inert use file-disarm on its own. This is a documented decision, not a tested guarantee: the macro-security default comes from LibreOffice's documented behaviour, not from a test here.

Warnings: `LAYOUT_APPROXIMATED` (HTML, Markdown, EPUB), `IMAGES_DROPPED` (to Markdown), `FORMULAS_DROPPED` and `SHEETS_SPLIT` (to CSV).
