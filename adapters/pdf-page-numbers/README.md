# pdf-page-numbers

Adds page numbers ("3", "Page 3", "3 of 10" or "Page 3 of 10") at the top or bottom of the page, left, centre or right, with optional text beside them (a title, "Confidential"). Numbering starts at any number and counts only the chosen pages, so a cover or contents page can be left out.

The text is drawn in Helvetica, which every PDF viewer provides, so no font is embedded and the file barely grows. Helvetica covers Western European characters only; others appear as "?" (`GLYPH_UNSUPPORTED`). Numbers are placed in what the viewer sees (the CropBox after rotation), like the watermark. The page content is otherwise unchanged; the shared saving rules apply (spec 13.2).

Engines: pikepdf, ReportLab (font metrics).
