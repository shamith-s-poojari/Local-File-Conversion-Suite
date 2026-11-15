# data-convert

Tables and structured data, in Python only: no LibreOffice (ADR 0003).

| From | To | How |
|---|---|---|
| CSV | JSON, YAML | One record per row, keyed by the first row (`firstRowIsHeader`, on by default), or a list of rows. Plain numbers become numbers; `007`, 16+ digit numbers and exponents stay text. |
| CSV | Excel, Markdown, HTML | The same table. Excel cells get numbers where the text is plainly a number. |
| JSON ↔ YAML | | The whole structure, keys in order. YAML dates become ISO text, binary becomes Base64, and comments are lost (`COMMENTS_DROPPED`). |
| JSON, YAML | CSV, Excel | A list of records becomes a table (columns in first-seen order); an object of lists becomes one table per key (sheets in Excel, files in CSV); anything else becomes key and value columns. Nested values are JSON text in their cell (`NESTED_DATA_FLATTENED`). |
| Excel | JSON, YAML, CSV | Every sheet, by its stored values: formulas give the value Excel last calculated (`FORMULAS_DROPPED`). CSV gives a file per sheet (`SHEETS_SPLIT`). |

The CSV separator is detected on input (comma, semicolon, tab or pipe) and
chosen on output (`csvDelimiter`). Text is read as UTF-8 or UTF-16 with a
byte order mark, else UTF-8, else Windows-1252.

## Safety

- Text that looks like a formula (`=`, `+`, `-`, `@`) is stored in Excel as
  text, never as a live formula.
- YAML is read with the safe loader: no tags that build objects. Every
  value is counted with aliases expanded, so a few lines can't expand to
  gigabytes (`E_LIMIT_EXCEEDED` past 5 million), and recursive aliases are
  refused (`E_INPUT_CORRUPT`).
- Workbooks are measured unpacked before they're opened (512 MB at most),
  and reading stops past 5 million cells.
- Excel's own limits (1,048,576 rows, 16,384 columns, 32,767 characters a
  cell) give `E_LIMIT_EXCEEDED` rather than a file Excel can't open.

CSV output isn't changed to defuse formulas: text from a workbook that
starts with `=` stays as it is, as in the source.
