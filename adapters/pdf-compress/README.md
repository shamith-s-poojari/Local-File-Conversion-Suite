# pdf-compress

Makes a PDF smaller, in two stages (ADR 0001 schedules it; ADR 0004 builds it):

1. **Pictures** (Balanced: 150 dpi, JPEG quality 75; Strong: 96 dpi, quality 55). Each 8-bit grey or colour picture is resampled so it's no sharper than that DPI across the document's largest page (`IMAGE_DOWNSCALED`), and re-encoded as JPEG. A picture is replaced only when that saves at least 10%. Pictures with colour-key masks, `/Decode` arrays, CMYK, palette or spot colour spaces, 1-bit scans, and JBIG2, CCITT or JPEG 2000 data are left alone. Transparency masks stay as they are (PDF lets a mask have its own size). Lossless skips this stage.
2. **Structure.** Unused resources are dropped and the file is rewritten under the shared saving rules (spec 13.2) with object streams and recompressed Flate streams.

If the result isn't smaller, the original is kept byte for byte (`KEPT_ORIGINAL`). An encrypted input is unlocked like every other tool's (the password is needed), so its output is the rewritten, unencrypted file even when that's larger.

Engines: pikepdf, Pillow.
