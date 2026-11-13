# markup-convert

Markdown, plain text, HTML and EPUB to each other and to PDF, in Python only:
no LibreOffice (ADR 0003).

| From | To | How |
|---|---|---|
| Markdown | HTML, PDF, text | markdown-it-py (CommonMark, tables, strikethrough). Raw HTML in the Markdown is shown as text, never run. |
| Text | HTML, Markdown, PDF | Paragraphs at blank lines; Markdown characters are escaped. |
| HTML | Markdown, text, PDF | lxml with network access off. Scripts, frames, plugins and stylesheet links are removed before layout. |
| EPUB | PDF, HTML, Markdown, text | PDF through MuPDF's EPUB layout; the rest from the chapters in spine order. Images inside the book are embedded in the HTML (up to 64 MB). |

PDFs are laid out by MuPDF's HTML engine on A4 or US Letter (`pageSize`),
with fallback fonts for CJK, Arabic, Devanagari and emoji.

Nothing is fetched. Images a document points at elsewhere (a web address, a
file next to it) are left out with `IMAGES_DROPPED`.

EPUBs are refused before anything is read when a member is over 200 MB or
the book unpacks to over 1 GB, and when they are DRM-encrypted (font
obfuscation is fine). XML is parsed without entities, DTDs or network.
