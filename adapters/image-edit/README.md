# image-edit

Rotates (90°, 180°, 270°), flips, resizes (by percentage, or to fit inside a width and height without enlarging) and turns images black and white. Each file keeps its format, so a JPG and a PNG give a JPG and a PNG: the core runs one part per format (ADR 0004). Animated GIF and WebP files keep every frame. EXIF orientation is applied first, and the camera details can be removed.

JPG, WebP (lossy) and AVIF are re-encoded at quality 92, which loses a little detail; PNG, BMP, TIFF, GIF and lossless WebP don't lose anything beyond the edit itself.

Engines: Pillow.
