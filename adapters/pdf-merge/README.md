# pdf-merge

Joins two or more PDFs, in the order the user chose, into one ([spec 13.3](../../docs/specs/phase-1.md#133-pdf-merge-pikepdf)).

| | |
|---|---|
| **Tier** | E: pages are copied, never re-rendered |
| **Engine** | pikepdf (qpdf), MPL-2.0 |
| **Inputs** | 2 to 200 PDFs, in merge order. The core resolves the `order` param into input order |

## Behavior

- Pages keep their size, rotation and boxes.
- **Bookmarks:**
  - `perFile` gives one top-level bookmark per file, titled from `fileTitles` (filled in by the core) or "Document N", with that file's own bookmarks nested under it.
  - `keep` copies every file's bookmarks to the top level.
  - `none` drops them.
- **Forms:** fields from different files are kept while their names stay unique. A later file whose field names clash with an earlier one is flattened, with `FORMS_FLATTENED`.
- **Metadata:** the result takes the title, author and other properties of the file at `metadataFrom` (default: the first). `-1` keeps none.

## Known limits

- Tag trees aren't merged: tagged input gives `TAGS_DROPPED`.
- XMP metadata isn't carried over; only the Info dictionary is.
- One open password applies to all inputs, because the protocol carries a single `secrets.password` per request. Unlock files with different passwords first.
- `/AcroForm /DR` default resources come from the first file that keeps its fields. Fields from later files rely on their own appearance streams.
