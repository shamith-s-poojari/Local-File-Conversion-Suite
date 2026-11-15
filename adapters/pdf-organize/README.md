# pdf-organize

Reorders, rotates, deletes and duplicates pages ([spec 13.5](../../docs/specs/phase-1.md#135-pdf-organize-pikepdf)).

| | |
|---|---|
| **Tier** | E |
| **Engine** | pikepdf (qpdf), MPL-2.0 |
| **Widget** | `pageOrganizer` |

`pages` is the final order, as `{source, rotate}` with 1-based `source` numbers:

- Pages left out are deleted.
- A page listed twice is duplicated.
- `rotate` is clockwise and relative to the page's current rotation.

## Behavior

- Bookmarks pointing at deleted pages are removed. Their children move up a level. All other bookmarks point at the page's first position in the new order.
- The form is kept when pages are only moved or rotated. If any page is deleted or duplicated, fields are flattened (`FORMS_FLATTENED`): a widget can't sit on two pages, and a field without a page breaks the form.
- Tagged input gives `TAGS_DROPPED`.
