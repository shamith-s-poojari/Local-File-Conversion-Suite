# image-convert

Converts between PNG, JPG, WebP, GIF, BMP, TIFF, AVIF and ICO, renders SVG, reads HEIC, and turns any of them into a one-page PDF. One output per input, named after the input.

| Behaviour | Detail |
|---|---|
| Orientation | Applied to the pixels (EXIF orientation), so results are upright everywhere |
| Transparency | Kept where the target supports it; JPG and BMP fill it with the chosen colour (`TRANSPARENCY_FLATTENED`) |
| Animation | Kept for GIF, WebP and AVIF targets; other targets take the first frame (`ANIMATION_FLATTENED`) |
| WebP | Lossless from PNG, GIF, BMP, TIFF and ICO; lossy at the chosen quality from JPG, HEIC and AVIF |
| GIF | 256 colours with dithering (`COLORS_REDUCED`) |
| ICO | Square, every standard size up to 256 px |
| PDF | One page at the image's DPI (72 if none). JPEG data is embedded as-is, never re-encoded |
| SVG | Rendered by MuPDF at twice its declared size; to PDF it stays vector |
| Limits | 200 megapixels per image (`E_LIMIT_EXCEEDED`) |

Engines: Pillow, pillow-heif, PyMuPDF (SVG and PDF), pikepdf (shared PDF saving rules).
