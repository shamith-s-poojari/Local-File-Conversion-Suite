# pdf-to-image

Renders PDF pages to PNG, JPG, WebP, TIFF, GIF or BMP with pdfium, or exports them as SVG with MuPDF (text as outlines). One image per page, named `<document>-page-<n>`; a one-page selection is named after the document. TIFF can collect every page into one multi-page file.

Password-protected PDFs use the password given when the file was opened. Pages are capped at 100 megapixels each; larger pages render at a lower resolution rather than failing.

Engines: pypdfium2 (raster), PyMuPDF (SVG), pikepdf (inspection), Pillow (encoding).
