# pdf-split

Splits one PDF into several ([spec 13.4](../../docs/specs/phase-1.md#134-pdf-split-pikepdf)).

| | |
|---|---|
| **Tier** | E |
| **Engine** | pikepdf (qpdf), MPL-2.0 |
| **Outputs** | `part-001.pdf`, `part-002.pdf`, …, padded to fit the total. The core adds the source file's name when saving |

## Modes

| Mode | Param | Result |
|---|---|---|
| `ranges` | `ranges`, e.g. `1-3, 5, 8-` | One output per comma-separated range |
| `everyN` | `n` | Parts of `n` pages; the last may be shorter |
| `eachPage` | | One output per page |
| `maxSize` | `maxSizeMb` | **Best effort.** Pages are grouped by the bytes of the streams they reference. A page bigger than the limit gets a part of its own |

Each part contains only the objects its pages need. Bookmarks that point into a part are kept in that part.

## Known limits

- Forms are flattened (`FORMS_FLATTENED`), because a field can't be split across files.
- Tagged input gives `TAGS_DROPPED`.
- `maxSize` estimates from stream lengths before compression and object streams, so parts usually end up a little under the limit.
