# pdf-to-document

PDF to Word, Excel, Markdown, HTML and plain text (ADR 0003). All tiers are
R (reconstructed): a PDF stores positioned text, not paragraphs, so structure
is inferred from the layout.

| To | How | Notes |
|---|---|---|
| DOCX | pdf2docx | Paragraphs, tables and images rebuilt. Pages are analysed ten at a time, so cancelling is quick and memory stays bounded. |
| XLSX | PyMuPDF table finder, openpyxl | One sheet per table. With no tables, one sheet of text lines (`LAYOUT_APPROXIMATED`). Numbers become numbers; cells that look like formulas stay text. |
| MD, HTML | PyMuPDF text with fonts | Headings from font sizes (up to three levels), bold and italic, tables. Images are left out (`IMAGES_DROPPED`). |
| TXT | PyMuPDF | The text layer in reading order, pages separated by a blank line. |

A PDF without a text layer (a scan) fails with `E_NO_TEXT` for the text
formats; Word still works, with `NO_TEXT_LAYER`, because pdf2docx keeps the
page images.

Parameters: `to`, optional `pages` (page ranges), and `fileTitles` from the core.
