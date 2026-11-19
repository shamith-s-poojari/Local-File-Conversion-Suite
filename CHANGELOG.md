# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Format conversion across 41+ formats: documents, images, audio, video, and data
- Full PDF toolkit: merge, split, organize, compress, page numbers, protect/unlock, watermark, redact
- Image tools: rotate, resize, compress to target size, animated GIF, images-to-PDF
- Audio tools: join, volume, fades, speed
- Video tools: join, rotate, speed, frame extraction
- Spreadsheet tools: combine and split CSV/Excel files
- Privacy inspection: detect hidden metadata, GPS, camera info, macros, scripts
- Remove hidden details from photos, PDFs, Office files, audio, and video
- Make safe to open: strip macros, scripts, embedded files with independent verification
- Per-job OS sandbox on macOS (Seatbelt) with startup self-test
- `lfcs` command-line interface with the same sandboxed processing as the desktop app
- Contact sheets for photos, PDF pages, and video
- Folder input/output with structure preservation
- Clipboard paste (images and tables) and copy (results)
- Zero network code — no telemetry, no updates, no crash reports
- Canary tests verifying no network connections during processing
- Mixed-format batch conversion (e.g., Word + photos → PDF in one go)

### Platform Status
- **macOS:** Fully functional with per-job Seatbelt sandbox
- **Linux:** Compiles; bubblewrap sandbox written but not yet verified on Linux
- **Windows:** Compiles; AppContainer sandbox compile-checked only

[Unreleased]: https://github.com/shamith-s-poojari/Local-File-Conversion-Suite/commits/feat/app
