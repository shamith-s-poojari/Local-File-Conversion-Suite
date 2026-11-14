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

Warnings: `LAYOUT_APPROXIMATED` (HTML, Markdown, EPUB), `IMAGES_DROPPED` (to Markdown), `FORMULAS_DROPPED` and `SHEETS_SPLIT` (to CSV).
