"""data-combine: CSV files and Excel workbooks into one file (ADR 0004).

- **Rows** stacks every table (each CSV, each non-empty sheet) into one.
  With column names, columns are matched by name in the order they first
  appear; a table without a column leaves it empty (``COLUMNS_MISMATCHED``).
- **Sheets** (Excel only) keeps each table as a sheet of its own.
"""

from __future__ import annotations

from typing import Any, ClassVar, Literal

from converter_sdk import (
    Adapter,
    AdapterError,
    JobContext,
    OutputFile,
    ResourceEstimate,
    ValidationReport,
    run_main,
)
from converter_sdk.convert import ConversionAdapter
from converter_sdk.tables import (
    MAX_CELLS,
    Table,
    read_tables,
    source_size,
    write_csv,
    write_xlsx,
)

MB = 1024 * 1024
SOURCE_COLUMN = "Source file"


def stack(
    tables: list[tuple[str, Table]], header: bool, add_source: bool
) -> tuple[list[list[Any]], bool]:
    """One table from many: (rows, whether the columns differed)."""
    rows: list[list[Any]] = []
    if not header:
        for source, t in tables:
            for r in t.rows:
                rows.append([source, *r] if add_source else list(r))
        return rows, False
    columns: list[str] = []
    seen: set[str] = set()
    for _, t in tables:
        for name in _names(t):
            if name not in seen:
                seen.add(name)
                columns.append(name)
    mismatched = False
    for source, t in tables:
        names = _names(t)
        if set(names) != seen:
            mismatched = True
        where = {n: i for i, n in reversed(list(enumerate(names)))}
        for r in t.rows[1:]:
            row = [r[where[c]] if c in where and where[c] < len(r) else None for c in columns]
            rows.append([source, *row] if add_source else row)
    head: list[Any] = [SOURCE_COLUMN, *columns] if add_source else list(columns)
    return [head, *rows], mismatched


def _names(t: Table) -> list[str]:
    """Column names from the first row; blank ones are numbered."""
    first = t.rows[0] if t.rows else []
    return [
        str(c).strip() if c not in (None, "") else f"Column {i}"
        for i, c in enumerate(first, start=1)
    ]


class DataCombineAdapter(Adapter):
    adapter_id: ClassVar[str] = "data-combine"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "H"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        sizes = [source_size(p) for p in ctx.inputs]
        return ValidationReport(metadata={"bytes": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        total = sum(metadata.get("bytes") or [0])
        # Every table is held at once; parsed values take far more room than text.
        return ResourceEstimate(
            ram_bytes=128 * MB + total * 24, disk_bytes=total * 3 + 16 * MB, cost_units=1
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = str(params.get("to", "xlsx"))
        header = params.get("firstRowIsHeader", True) is not False
        add_source = bool(params.get("addSource", False))
        warned: set[str] = set()

        def warn(code: str) -> None:
            if code not in warned:
                warned.add(code)
                ctx.warn(code)

        tables: list[tuple[str, Table]] = []
        cells = 0
        ctx.stage("read", 1, 2)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            title = ConversionAdapter.title(params, i)
            found = read_tables(ctx, path, warn)
            for t in found:
                if not t.rows:
                    continue
                cells += sum(len(r) for r in t.rows)
                if cells > MAX_CELLS:
                    raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "cells", "max": MAX_CELLS})
                if to == "xlsx" and t.text:
                    # CSV numbers become numbers in Excel; Excel cells stay as they are.
                    t = Table(t.name, t.typed())
                source = title if len(found) == 1 else f"{title} - {t.name}"
                tables.append((source, t))
            ctx.progress("read", i + 1, len(ctx.inputs))

        ctx.stage("write", 2, 2)
        out = ctx.output_path(f"combined.{to}")
        if to == "xlsx" and params.get("mode", "rows") == "sheets":
            sheets = [Table(source, t.rows) for source, t in tables]
            write_xlsx(ctx, out, sheets, stage="write")
            return [OutputFile(out)]
        rows, mismatched = stack(tables, header, add_source)
        if mismatched:
            warn("COLUMNS_MISMATCHED")
        if to == "csv":
            write_csv(out, rows)
        else:
            write_xlsx(ctx, out, [Table("Combined", rows)], stage="write")
        return [OutputFile(out)]


if __name__ == "__main__":
    run_main(DataCombineAdapter)
