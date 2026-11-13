# images-to-pdf

Puts several images into one PDF, one page per image, in the order chosen with the file-order widget. Pages can match each image or be A4 or US Letter (landscape images get landscape pages), with an optional margin. JPEG data is embedded without re-encoding; other formats go in as lossless PNG. EXIF orientation is applied.

Engines: Pillow, pillow-heif, PyMuPDF, pikepdf (shared PDF saving rules).
