"""data-convert: CSV, JSON, YAML and Excel, without LibreOffice (ADR 0003).

- **Tables** (CSV, Excel sheets, a list of records in JSON or YAML) keep
  their rows and columns. With ``firstRowIsHeader`` (the default), a table
  becomes a list of records keyed by its first row, and back.
- **Structure.** JSON and YAML convert into each other whole. Values that
  have structure of their own (a list inside a record) can't fit a cell, so
  they're written there as JSON text (``NESTED_DATA_FLATTENED``).
- **Numbers.** CSV cells that are plainly numbers become numbers; anything
  that could lose information as a number (``007``, 16+ digits, ``1e5``)
  stays text.
- **Formulas never come alive.** Text that starts like a formula (``=``,
  ``+``, ``-``, ``@``) is stored in Excel as text. Excel inputs are read
  for their stored values (``FORMULAS_DROPPED`` when they had formulas).
- **YAML** is read safely (no tags that build objects); aliases are
  expanded within a limit, so a few lines can't expand to gigabytes, and
  comments are lost (``COMMENTS_DROPPED``).
"""

from __future__ import annotations

import base64
import datetime as dt
import html as html_lib
import json
import math
import re
from pathlib import Path
from typing import Any, ClassVar

from converter_sdk import AdapterError, JobContext, OutputFile, ResourceEstimate, run_main
from converter_sdk.adapter import ValidationReport
from converter_sdk.convert import ConversionAdapter, Tier, source_format
from converter_sdk.markup import escape_markdown, html_document, read_text
from converter_sdk.tables import (
    DELIMITERS,
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
EDGES: dict[tuple[str, str], Tier] = {
    ("csv", "json"): "H", ("csv", "yaml"): "H", ("csv", "xlsx"): "H",
    ("csv", "md"): "H", ("csv", "html"): "H",
    ("json", "yaml"): "H", ("json", "csv"): "R", ("json", "xlsx"): "R",
    ("yaml", "json"): "H", ("yaml", "csv"): "R", ("yaml", "xlsx"): "R",
    ("xlsx", "json"): "R", ("xlsx", "yaml"): "R", ("xlsx", "csv"): "R",
}  # fmt: skip
MAX_NODES = 5_000_000
"""Values in a JSON or YAML document with every alias expanded."""
FORMULA_START = ("=", "+", "-", "@", "\t", "\r")


class DataConvertAdapter(ConversionAdapter):
    adapter_id: ClassVar[str] = "data-convert"
    EDGES = EDGES

    # ------------------------------------------------------------ validate

    def validate_sources(
        self, ctx: JobContext, params: dict[str, Any], to: str
    ) -> ValidationReport:
        sizes = [source_size(p) for p in ctx.inputs]
        return ValidationReport(metadata={"bytes": sizes})

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        largest = max(metadata.get("bytes") or [0])
        # Parsed values take far more room than their text.
        return ResourceEstimate(
            ram_bytes=128 * MB + largest * 24, disk_bytes=largest * 4 + 16 * MB, cost_units=1
        )

    # ------------------------------------------------------------- execute

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        to = self.target(params)
        self.use_tiers(ctx, to)
        self._warned: set[str] = set()
        header = params.get("firstRowIsHeader", True) is not False
        outputs: list[OutputFile] = []
        total = len(ctx.inputs)
        for i, path in enumerate(ctx.inputs):
            ctx.check_cancelled()
            ctx.stage("convert", i + 1, total)
            src = source_format(path)
            title = self.title(params, i)
            if src in ("csv", "xlsx"):
                tables = read_tables(ctx, path, lambda code: self._warn(ctx, code))
                if to in ("json", "yaml"):
                    outputs.append(
                        self._write_data(ctx, title, to, _tables_to_data(tables, header))
                    )
                else:
                    outputs += self._write_tables(ctx, title, to, tables, header, params)
            else:
                data = self._read_data(path, src)
                if to in ("json", "yaml"):
                    outputs.append(self._write_data(ctx, title, to, data))
                else:
                    tables = self._data_to_tables(data, title)
                    outputs += self._write_tables(ctx, title, to, tables, True, params)
        return outputs

    # ------------------------------------------------------------- reading

    def _read_data(self, path: Path, src: str) -> Any:
        text = read_text(path)
        if src == "json":
            try:
                data = json.loads(text)
            except (ValueError, RecursionError):
                raise AdapterError("E_INPUT_CORRUPT") from None
        else:
            data = _load_yaml(text)
            if re.search(r"(?m)(^|\s)#", text):
                self._pending_comments = True
        _check_size(data)
        return data

    # ------------------------------------------------------------- shaping

    def _data_to_tables(self, data: Any, title: str) -> list[Table]:
        """JSON or YAML as one or more tables."""
        if isinstance(data, dict) and data and all(isinstance(v, list) for v in data.values()):
            return [Table(str(k), self._list_rows(v)) for k, v in data.items()]
        if isinstance(data, list):
            return [Table(title, self._list_rows(data))]
        if isinstance(data, dict):
            return [
                Table(
                    title, [["key", "value"]] + [[str(k), self._cell(v)] for k, v in data.items()]
                )
            ]
        return [Table(title, [[self._cell(data)]])]

    def _list_rows(self, items: list[Any]) -> list[list[Any]]:
        if items and all(isinstance(x, dict) for x in items):
            keys: dict[str, None] = {}
            for item in items:
                for k in item:
                    keys.setdefault(str(k), None)
            header = list(keys)
            rows: list[list[Any]] = [list(header)]
            for item in items:
                by_name = {str(k): v for k, v in item.items()}
                rows.append([self._cell(by_name.get(k)) for k in header])
            return rows
        return [
            [self._cell(c) for c in x] if isinstance(x, list) else [self._cell(x)] for x in items
        ]

    def _cell(self, value: Any) -> Any:
        if isinstance(value, dict | list | tuple | set):
            self._nested = True
            return json.dumps(_jsonable(value), ensure_ascii=False, separators=(",", ":"))
        return _jsonable(value)

    # ------------------------------------------------------------- writing

    def _write_data(self, ctx: JobContext, title: str, to: str, data: Any) -> OutputFile:
        self._flush_warnings(ctx)
        out = ctx.output_path(f"{file_name(title)}.{to}")
        clean = _jsonable(data)
        if to == "json":
            text = json.dumps(clean, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
        else:
            import yaml  # noqa: PLC0415

            dumper = getattr(yaml, "CSafeDumper", yaml.SafeDumper)
            text = yaml.dump(
                clean, Dumper=dumper, sort_keys=False, allow_unicode=True, default_flow_style=False
            )
        out.write_text(text, "utf-8", newline="\n")
        return OutputFile(out)

    def _write_tables(
        self,
        ctx: JobContext,
        title: str,
        to: str,
        tables: list[Table],
        header: bool,
        params: dict[str, Any],
    ) -> list[OutputFile]:
        self._flush_warnings(ctx)
        base = file_name(title)
        if to == "csv":
            delimiter = DELIMITERS.get(str(params.get("csvDelimiter") or "comma"), ",")
            if len(tables) > 1:
                self._warn(ctx, "SHEETS_SPLIT")
            outs = []
            used: set[str] = set()
            for t in tables:
                name = base if len(tables) == 1 else unique(f"{base}-{file_name(t.name)}", used)
                out = ctx.output_path(f"{name}.csv")
                write_csv(out, t.rows, delimiter)
                outs.append(OutputFile(out))
            return outs
        out = ctx.output_path(f"{base}.{to}")
        if to == "xlsx":
            write_xlsx(ctx, out, tables)
        elif to == "md":
            parts = []
            for t in tables:
                if len(tables) > 1:
                    parts.append(f"## {escape_markdown(t.name)}")
                parts.append(_md_table(t.rows, header))
            out.write_text("\n\n".join(parts) + "\n", "utf-8", newline="\n")
        else:  # html
            parts = []
            for t in tables:
                if len(tables) > 1:
                    parts.append(f"<h2>{html_lib.escape(t.name)}</h2>")
                parts.append(_html_table(t.rows, header))
            out.write_text(html_document("\n".join(parts), title), "utf-8", newline="\n")
        return [OutputFile(out)]

    # ------------------------------------------------------------ warnings

    def _warn(self, ctx: JobContext, code: str) -> None:
        if code not in self._warned:
            self._warned.add(code)
            ctx.warn(code)

    def _flush_warnings(self, ctx: JobContext) -> None:
        if getattr(self, "_nested", False):
            self._warn(ctx, "NESTED_DATA_FLATTENED")
        if getattr(self, "_pending_comments", False):
            self._warn(ctx, "COMMENTS_DROPPED")


# ------------------------------------------------------------------- values


def _jsonable(value: Any) -> Any:
    """Plain JSON values: dates as ISO text, bytes as Base64, keys as text."""
    if isinstance(value, dict):
        return {_key(k): _jsonable(v) for k, v in value.items()}
    if isinstance(value, list | tuple | set):
        return [_jsonable(v) for v in value]
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if isinstance(value, dt.datetime | dt.date | dt.time):
        return value.isoformat()
    if isinstance(value, dt.timedelta):
        return value.total_seconds()
    if isinstance(value, bytes):
        return base64.b64encode(value).decode("ascii")
    if value is None or isinstance(value, bool | int | float | str):
        return value
    return str(value)


def _key(k: Any) -> str:
    if isinstance(k, bool):
        return "true" if k else "false"
    if k is None:
        return "null"
    return k if isinstance(k, str) else str(_jsonable(k))


def _tables_to_data(tables: list[Table], header: bool) -> Any:
    def one(t: Table) -> list[Any]:
        rows = t.typed()
        if not header or not rows:
            return [list(r) for r in rows]
        keys = _header_keys(rows[0])
        records = []
        for r in rows[1:]:
            padded = list(r) + [None] * (len(keys) - len(r))
            record = dict(zip(keys, padded, strict=False))
            for extra, value in enumerate(r[len(keys) :], start=len(keys) + 1):
                record[f"column{extra}"] = value
            records.append(record)
        return records

    if len(tables) == 1:
        return one(tables[0])
    return {t.name: one(t) for t in tables}


def _header_keys(row: list[Any]) -> list[str]:
    keys: list[str] = []
    for i, c in enumerate(row, start=1):
        base = str(_jsonable(c)).strip() if c not in (None, "") else f"column{i}"
        name, n = base, 2
        while name in keys:
            name, n = f"{base} ({n})", n + 1
        keys.append(name)
    return keys


def _load_yaml(text: str) -> Any:
    import yaml  # noqa: PLC0415

    loader = getattr(yaml, "CSafeLoader", yaml.SafeLoader)
    try:
        docs = list(yaml.load_all(text, Loader=loader))
    except (yaml.YAMLError, RecursionError, ValueError):
        raise AdapterError("E_INPUT_CORRUPT") from None
    if not docs:
        raise AdapterError("E_INPUT_CORRUPT", {"reason": "empty"})
    return docs[0] if len(docs) == 1 else docs


def _check_size(data: Any) -> None:
    """Counts values with aliases expanded; refuses loops and bombs."""
    count = 0
    open_ids: set[int] = set()

    def walk(v: Any, depth: int) -> None:
        nonlocal count
        count += 1
        if count > MAX_NODES:
            raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "values", "max": MAX_NODES})
        if depth > 500:
            raise AdapterError("E_INPUT_CORRUPT", {"reason": "tooDeep"})
        if isinstance(v, dict | list):
            if id(v) in open_ids:
                raise AdapterError("E_INPUT_CORRUPT", {"reason": "recursive"})
            open_ids.add(id(v))
            for child in v.values() if isinstance(v, dict) else v:
                walk(child, depth + 1)
            open_ids.discard(id(v))

    walk(data, 0)


def _cell_text(value: Any) -> str:
    return "" if value is None else re.sub(r"\s*\n\s*", " ", csv_text(value))


def _md_table(rows: list[list[Any]], header: bool) -> str:
    width = max((len(r) for r in rows), default=1) or 1
    cells = [[escape_markdown(_cell_text(c)) for c in r] + [""] * (width - len(r)) for r in rows]
    if header and cells:
        head, body = cells[0], cells[1:]
    else:
        head, body = [f"Column {i + 1}" for i in range(width)], cells
    lines = ["| " + " | ".join(head) + " |", "|" + "---|" * width]
    lines += ["| " + " | ".join(r) + " |" for r in body]
    return "\n".join(lines)


def _html_table(rows: list[list[Any]], header: bool) -> str:
    def row(cells: list[Any], tag: str) -> str:
        return (
            "<tr>"
            + "".join(f"<{tag}>{html_lib.escape(_cell_text(c))}</{tag}>" for c in cells)
            + "</tr>"
        )

    if header and rows:
        head = f"<thead>{row(rows[0], 'th')}</thead>"
        body = rows[1:]
    else:
        head, body = "", rows
    return f"<table>{head}<tbody>{''.join(row(r, 'td') for r in body)}</tbody></table>"


if __name__ == "__main__":
    run_main(DataConvertAdapter)
