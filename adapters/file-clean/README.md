# file-clean

Removes hidden details from pictures, PDFs and Office files, one output per input, each in its own format (ADR 0004). Names are `{title}-clean.{ext}`.

- **JPEG**: EXIF, XMP, IPTC, thumbnails and comments are cut out of the file; the compressed picture is copied byte for byte, so pixels don't change. A picture stored sideways (EXIF orientation) is first turned upright and re-saved at quality 95, with the warning `REENCODED`. Colour profiles (ICC) stay.
- **PNG**: text, EXIF and time chunks are dropped. **WebP**: the EXIF and XMP chunks are dropped. Neither is re-encoded.
- **TIFF, AVIF**: re-saved by Pillow (TIFF losslessly, AVIF at quality 95) without EXIF or XMP, with the warning `REENCODED`. Only the first frame of an AVIF is kept.
- **PDF** (pikepdf): document information, XMP, page-piece data and thumbnails are deleted, and so is the metadata (EXIF, XMP, IPTC, comments) inside embedded JPEG pictures; the compressed picture is copied byte for byte. `removeComments` (default on) drops annotations other than links and form fields; `removeHiddenLayers` (default off) removes layers that are switched off together with what is drawn on them.
- **Word, Excel, PowerPoint**:
  - Author, last editor, revision, print and edit dates, company, manager, template, custom properties and the thumbnail are removed. Of `app.xml` only the counts (pages, words, slides) stay: the program name is left out, not replaced by a made-up one.
  - Pictures in `word/media`, `xl/media` and `ppt/media` lose EXIF, XMP and text chunks, at byte level (TIFF is re-saved by Pillow). A picture over 64 MB fails the job with `E_LIMIT_EXCEEDED`, as does an XML part over 64 MB that has to change. EXIF orientation is removed with the rest, without turning the picture.
  - Every ZIP entry is dated 1980-01-01.
  - Relationships that point outside the file (an attached template, linked pictures or files, links to local files) are deleted together with what used them. Plain web and mail links stay. Links to other workbooks (`xl/externalLinks`) in an xlsx stay, because the formulas need them; their paths are not removed.
  - `removeComments` drops comment parts. Tracked changes are not accepted: the text stays and the job warns `TRACKED_CHANGES_KEPT`, but each change's author becomes "Author" and its date is removed.
  - The package is checked for zip bombs (`markup.check_zip`) before it is read.

`DETAILS_REMOVED` is warned when something was removed. Each output is read again, and the job fails with `E_INTERNAL` (`detailsRemain`) if a place, device, author, date or outside link is still found in it.

Engines: Pillow, pikepdf, lxml.
