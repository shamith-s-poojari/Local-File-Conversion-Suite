"""pdf-split: one PDF into several (spec section 13.4)."""

from __future__ import annotations

from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import AdapterError, JobContext, OutputFile, run_main
from converter_sdk.pdf import (
    OpenedInput,
    PdfAdapter,
    RangeError,
    copy_outline,
    field_names,
    flatten_forms,
    is_tagged,
    param_issue,
    parse_page_ranges,
    save_pdf,
)
from converter_sdk.protocol import ValidationIssue
from pikepdf import Name

MB = 1024 * 1024
MAX_OUTPUTS = 10_000
"""The protocol's limit on outputs per job."""


def plan_parts(params: dict[str, Any], total: int) -> list[list[int]]:
    """0-based page lists, one per output, for every mode except ``maxSize``."""
    mode = params.get("mode", "ranges")
    if mode == "ranges":
        ranges = parse_page_ranges(str(params.get("ranges", "")), total)
        return [list(range(first - 1, last)) for first, last in ranges]
    if mode == "everyN":
        n = int(params["n"])
        return [list(range(s, min(s + n, total))) for s in range(0, total, n)]
    if mode == "eachPage":
        return [[i] for i in range(total)]
    raise ValueError(mode)


def _footprint(obj: Any, seen: set[tuple[int, int]], depth: int = 0) -> int:
    """Approximate bytes an object pulls into a file: raw stream lengths of
    everything reachable that ``seen`` doesn't already hold. Pages are
    skipped, so /Parent and /P links don't walk the whole document."""
    if depth > 64:
        return 0
    if not isinstance(obj, pikepdf.Object):  # numbers and booleans come back as Python values
        return 16
    if obj.is_indirect:
        if obj.objgen in seen:
            return 0
        seen.add(obj.objgen)
    size = 0
    if isinstance(obj, pikepdf.Stream):
        size += int(obj.get(Name.Length, 0)) if isinstance(obj.get(Name.Length), int) else 0
        size += 64
        children = [v for _, v in obj.stream_dict.items()]
    elif isinstance(obj, pikepdf.Dictionary):
        if obj.get(Name.Type) == Name.Page and depth > 0:
            return 0
        size += 32
        children = [v for k, v in obj.items() if k not in ("/Parent", "/P")]
    elif isinstance(obj, pikepdf.Array):
        children = list(obj)
    else:
        return 16
    for child in children:
        size += _footprint(child, seen, depth + 1)
    return size


def plan_by_size(ctx: JobContext, pdf: pikepdf.Pdf, limit: int) -> list[list[int]]:
    """Greedy, best-effort grouping under ``limit`` bytes. A single page larger
    than the limit gets a part of its own, as the spec allows."""
    parts: list[list[int]] = []
    current: list[int] = []
    seen: set[tuple[int, int]] = set()
    used = 1024  # header, trailer, page tree
    for i, page in enumerate(pdf.pages):
        ctx.check_cancelled()
        trial = set(seen)
        cost = _footprint(page.obj, trial)
        if current and used + cost > limit:
            parts.append(current)
            current, used = [], 1024
            trial = set()
            cost = _footprint(page.obj, trial)
        current.append(i)
        seen = trial
        used += cost
    if current:
        parts.append(current)
    return parts


class PdfSplitAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-split"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        if len(inputs) != 1:
            return [param_issue("inputs", "exactlyOne")]
        total = inputs[0].pages
        mode = params.get("mode", "ranges")
        if mode not in ("ranges", "everyN", "eachPage", "maxSize"):
            return [param_issue("mode", "invalid")]
        if mode == "maxSize":
            size = params.get("maxSizeMb")
            if not isinstance(size, int) or size < 1:
                return [param_issue("maxSizeMb", "required")]
            return []
        if mode == "everyN":
            n = params.get("n")
            if not isinstance(n, int) or n < 1:
                return [param_issue("n", "required")]
        try:
            parts = plan_parts(params, total)
        except RangeError as e:
            return [param_issue("ranges", e.reason, pages=total)]
        if len(parts) > MAX_OUTPUTS:
            return [ValidationIssue(code="E_LIMIT_EXCEEDED", detail={"limit": "outputs"})]
        return []

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        ctx.stage("plan", 1, 2)
        with ctx.open_pdf(ctx.inputs[0]) as src:
            self.warn_shared(ctx, [src])
            if is_tagged(src):
                ctx.warn("TAGS_DROPPED")
            if field_names(src):
                # Fields span the whole document; a part can't keep a valid form.
                flatten_forms(src)
                ctx.warn("FORMS_FLATTENED")
            total = len(src.pages)
            if params.get("mode") == "maxSize":
                parts = plan_by_size(ctx, src, int(params["maxSizeMb"]) * MB)
            else:
                parts = plan_parts(params, total)
            if len(parts) > MAX_OUTPUTS:
                raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "outputs"})

            ctx.stage("write", 2, 2)
            width = max(3, len(str(len(parts))))
            outputs: list[OutputFile] = []
            for n, pages in enumerate(parts, start=1):
                ctx.check_cancelled()
                part = pikepdf.new()
                for i in pages:
                    part.pages.append(src.pages[i])
                for page in part.pages:
                    if Name.StructParents in page.obj:
                        del page.obj[Name.StructParents]
                page_map: dict[int, int] = {}
                for j, i in enumerate(pages):
                    page_map.setdefault(i, j)
                with part.open_outline() as outline:
                    copy_outline(src, page_map, outline.root)
                for key, value in src.docinfo.items():
                    if isinstance(value, pikepdf.String) and key != "/Producer":
                        part.docinfo[key] = pikepdf.String(str(value))
                out = ctx.output_path(f"part-{n:0{width}d}.pdf")
                count = save_pdf(ctx, part, out)
                part.close()
                outputs.append(OutputFile(out, pages=count))
                ctx.progress("write", n, len(parts))
            # A bookmark outside a part is expected, not lost. Warn only about
            # bookmarks whose destination can't be resolved at all.
            if _unresolved(src):
                ctx.warn("OUTLINE_DROPPED")
            return outputs


def _unresolved(src: pikepdf.Pdf) -> bool:
    every = {i: i for i in range(len(src.pages))}
    return copy_outline(src, every, []) > 0


if __name__ == "__main__":
    run_main(PdfSplitAdapter)
