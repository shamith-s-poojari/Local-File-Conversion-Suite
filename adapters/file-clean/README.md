# file-clean

Removes hidden details from pictures, PDFs and Office files, one output per input, each in its own format (ADR 0004). Names are `{title}-clean.{ext}`.

- **JPEG**: EXIF, XMP, IPTC, thumbnails and comments are cut out of the file; the compressed picture is copied byte for byte, so pixels don't change. A picture stored sideways (EXIF orientation) is first turned upright and re-saved at quality 95, with the warning `REENCODED`. Colour profiles (ICC) stay.
- **PNG**: text, EXIF and time chunks are dropped. **WebP**: the EXIF and XMP chunks are dropped. Neither is re-encoded.
- **TIFF, AVIF**: re-saved by Pillow (TIFF losslessly, AVIF at quality 95) without EXIF or XMP, with the warning `REENCODED`. Only the first frame of an AVIF is kept.
- **PDF** (pikepdf): document information, XMP, page-piece data and thumbnails are deleted. `removeComments` (default on) drops annotations other than links and form fields; `removeHiddenLayers` (default off) removes layers that are switched off together with what is drawn on them.
- **Word, Excel, PowerPoint**: author, last editor, revision, print and edit dates, company, manager, template, custom properties and the thumbnail are removed; `removeComments` drops comment parts. Tracked changes are not accepted: the file keeps them and the job warns `TRACKED_CHANGES_KEPT`.

`DETAILS_REMOVED` is warned when something was removed. Each output is read again, and the job fails with `E_INTERNAL` (`detailsRemain`) if a place, device, author or date is still found in it.

Engines: Pillow, pikepdf, lxml.
