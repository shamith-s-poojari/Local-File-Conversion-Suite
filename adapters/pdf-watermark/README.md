# pdf-watermark

Stamps text on selected pages ([spec 13.7](../../docs/specs/phase-1.md#137-pdf-watermark-pikepdf--reportlab)).

| | |
|---|---|
| **Tier** | E: page content is kept; the stamp is added on top or underneath |
| **Engines** | pikepdf (MPL-2.0), ReportLab (BSD) |

## Behavior

- One stamp is built per distinct displayed page size and shared by every page of that size, so large jobs grow only slightly.
- The stamp is placed in what the viewer sees: the CropBox, after `/Rotate`. Corners and the centre are correct on 0, 90, 180 and 270° pages, and with a non-zero CropBox origin. Placement is checked by rendering in the tests.
- `over` wraps the existing content in `q … Q`, so its graphics state can't leak into the stamp. `under` draws the stamp first.
- Pages outside `pages` are untouched.
- A watermark is a label. It doesn't stop anyone from removing it, and the UI should say so.

## Known limits

- **Text only.** Image watermarks are first on the week 6 cut list, and `type: image` is refused for now.
- **Font:** packaging is meant to bundle Noto Sans at `engines/fonts/NotoSans-Regular.ttf` (spec 13.7). Until it does, Helvetica is used: only Windows-1252 characters can be shown, and the font isn't embedded. Unsupported characters are drawn as `?` and raise `GLYPH_UNSUPPORTED`.
