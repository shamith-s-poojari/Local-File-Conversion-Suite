"""data-split: a CSV file or Excel workbook into several of the same
format (ADR 0004), every N rows or one file per value of a column. The
column names are repeated at the top of every part. A workbook splits its
first non-empty sheet (``ONLY_FIRST_SHEET`` when it has more)."""

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
from converter_sdk.convert import ConversionAdapter, issue, source_format
from converter_sdk.tables import (
    Table,
    csv_text,
    file_name,
    read_tables,
    source_size,
    unique,
    write_csv,
    write_xlsx,
)

MB = 1024 * 1024
MAX_PARTS = 1000
"""More files than this from one input is almost certainly a mistake."""


def column_index(header: list[Any], name: str) -> int | None:
    """The first column whose name matches, ignoring case and spaces."""
    want = name.strip().casefold()
    for i, c in enumerate(header):
        if csv_text(c).strip().casefold() == want:
            return i
    return None


def split_rows(rows: list[list[Any]], params: dict[str, Any]) -> list[tuple[str, list[list[Any]]]]:
    """(name suffix, rows) for each part, the header repeated in each."""
    header = params.get("firstRowIsHeader", True) is not False
    head, body = (rows[:1], rows[1:]) if header and rows else ([], rows)
    if params.get("mode", "rows") == "column":
        index = column_index(head[0] if head else [], str(params.get("column", "")))
        if index is None:
            raise AdapterError("E_INTERNAL", {"reason": "columnNotFound"}, retryable=False)
        groups: dict[str, list[list[Any]]] = {}
        for r in body:
            value = csv_text(r[index]).strip() if index < len(r) else ""
            groups.setdefault(value or "(empty)", []).append(r)
        if len(groups) > MAX_PARTS:
            raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "parts", "max": MAX_PARTS})
        return [(value, head + part) for value, part in groups.items()]
    size = max(1, int(params.get("rows", 1000)))
    count = max(1, -(-len(body) // size))
    if count > MAX_PARTS:
        raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "parts", "max": MAX_PARTS})
    return [(f"part-{n + 1}", head + body[n * size : (n + 1) * size]) for n in range(count)]


class DataSplitAdapter(Adapter):
    adapter_id: ClassVar[str] = "data-split"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "H"

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        if params.get("mode") == "column":
            if not str(params.get("column", "")).strip():
                return ValidationReport(errors=[issue("column", "required")])
            if params.get("firstRowIsHeader", True) is False:
                return ValidationReport(errors=[issue("firstRowIsHeader", "neededForColumn")])
        sizes = [source_size(p) for p in ctx.inputs]
        if params.get("mode") == "column":
            # Shown next to the field, before anything runs.
            for p in ctx.inputs:
                ctx.check_cancelled()
                tables = [t for t in read_tables(ctx, p, lambda _code: None) if t.rows]
                if tables and column_index(tables[0].rows[0], str(params["column"])) is None:
                    return ValidationReport(errors=[issue("column", "notFound")])
        return ValidationReport(metadata={"bytes": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        largest = max(metadata.get("bytes") or [0])
        return ResourceEstimate(
            ram_bytes=128 * MB + largest * 30, disk_bytes=largest * 3 + 16 * MB, cost_units=1
        )

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        warned: set[str] = set()

        def warn(code: str) -> None:
            if code not in warned:
                warned.add(code)
                ctx.warn(code)

        outputs: list[OutputFile] = []
        total = len(ctx.inputs)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("split", i + 1, total)
            fmt = source_format(path)
            tables = [t for t in read_tables(ctx, path, warn) if t.rows]
            if not tables:
                raise AdapterError("E_INPUT_CORRUPT", {"reason": "empty"})
            if len(tables) > 1:
                warn("ONLY_FIRST_SHEET")
            table = tables[0]
            base = file_name(ConversionAdapter.title(params, i))
            used: set[str] = set()
            parts = split_rows(table.rows, params)
            for n, (suffix, rows) in enumerate(parts):
                ctx.check_cancelled()
                name = unique(f"{base}-{file_name(suffix)}", used, limit=200)
                out = ctx.output_path(f"{name}.{fmt}")
                if fmt == "csv":
                    write_csv(out, rows)
                else:
                    write_xlsx(ctx, out, [Table(table.name, rows)], stage="split")
                outputs.append(OutputFile(out))
                ctx.progress("split", n + 1, len(parts))
        return outputs


if __name__ == "__main__":
    run_main(DataSplitAdapter)
