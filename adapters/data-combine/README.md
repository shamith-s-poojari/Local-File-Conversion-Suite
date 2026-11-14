# data-combine

Combines two or more CSV files and Excel workbooks into one Excel workbook or CSV file, in the order chosen with the file-order widget.

- **Stacking the rows** puts every table (each CSV file, and each non-empty sheet of a workbook) into one. When the first row holds column names, columns are matched by name in the order they first appear, the names appear once at the top, and a table that lacks a column leaves it empty (`COLUMNS_MISMATCHED`). An optional first column names the file (and sheet) each row came from.
- **Keeping each file as its own sheet** (Excel only) copies each table to a sheet named after its file.

CSV numbers become Excel numbers on the same rules as data-convert; text that looks like a formula stays text. Excel inputs are read for their stored values (`FORMULAS_DROPPED`).

Engines: openpyxl.
