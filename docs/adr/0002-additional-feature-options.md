# ADR 0002: Additional feature options (not adopted; number kept)

- Status: withdrawn
- Date: 2026-11-15 (the original proposal was written on 2026-11-09)

## What happened

ADR 0002 was proposed on 2026-11-09 as a list of further features beyond ADR 0001: tiered options reusing the approved engines, such as sanitize, metadata, page numbers, interleave, extract pages, repair and flatten for PDFs, a command line, tool presets and a privacy panel. It was written against the `feat/scheduler` branch, when `main` held only the first two tasks.

It was never accepted. The proposal stayed on the branch `docs/adr-0002-feature-options` and was never merged. Its status there is "Proposed".

The work that followed was decided in other ADRs instead:

- [ADR 0003](0003-all-format-conversion.md): conversion between every common format, including audio and video.
- [ADR 0004](0004-tools-for-every-kind-of-file.md): everyday tools for images, audio, video, data files and PDFs, including `pdf-compress` and `pdf-page-numbers`.
- [ADR 0005](0005-safety-privacy-and-convenience.md): what a file reveals, removing hidden details, making files safe to open, the `lfcs` command line, folders and the clipboard.

Some of the options in the original proposal ended up built that way (page numbers, a command line, a privacy page). Others were not (interleave, repair, compare, recipes, PDF/A, form filling). None of them is approved by this record.

## Decision

The number 0002 stays reserved so that the numbering of ADRs 0003 to 0005 and the links to them don't change. Anyone who wants an option from the old proposal writes a new ADR with the next free number. The old text is still readable on the branch `docs/adr-0002-feature-options` (`git show docs/adr-0002-feature-options:docs/adr/0002-additional-feature-options.md`), but it describes a plan that no longer matches the code, so check it against the current state first.
