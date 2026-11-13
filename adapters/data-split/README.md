# data-split

Splits each CSV file or Excel workbook into several files of the same format (ADR 0004):

- **Every so many rows** (1 to 1,000,000 per file): `name-part-1`, `name-part-2`, …
- **By the values in a column**, named by its heading: one file per value, named after it (`sales-Oslo.csv`); rows with no value go to `(empty)`.

When the first row holds column names, they're repeated at the top of every file; splitting by a column needs them. A workbook splits its first non-empty sheet (`ONLY_FIRST_SHEET` if it has more), and is read for its stored values (`FORMULAS_DROPPED`). One input can make at most 1,000 files (`E_LIMIT_EXCEEDED`). Cells are copied as they are: CSV text stays text, Excel values keep their types.

Engines: openpyxl.
