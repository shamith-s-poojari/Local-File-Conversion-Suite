"""pdf-organize: reorder, rotate, delete and duplicate pages (spec section 13.5)."""

from __future__ import annotations

from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import JobContext, OutputFile, run_main
from converter_sdk.pdf import (
    OpenedInput,
    PdfAdapter,
    copy_into,
    copy_outline,
    field_names,
    flatten_forms,
    is_tagged,
    param_issue,
    save_pdf,
)
from converter_sdk.protocol import ValidationIssue
from pikepdf import Name

ROTATIONS = (0, 90, 180, 270)


class PdfOrganizeAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-organize"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        if len(inputs) != 1:
            return [param_issue("inputs", "exactlyOne")]
        total = inputs[0].pages
        pages = params.get("pages")
        if not isinstance(pages, list) or not pages:
            return [param_issue("pages", "empty")]
        if len(pages) > self.max_pages * 10:
            return [ValidationIssue(code="E_LIMIT_EXCEEDED", detail={"limit": "pages"})]
        for n, entry in enumerate(pages):
            source = entry.get("source") if isinstance(entry, dict) else None
            if not isinstance(source, int) or not 1 <= source <= total:
                return [param_issue("pages", "outOfBounds", index=n, pages=total)]
            if entry.get("rotate", 0) not in ROTATIONS:
                return [param_issue("pages", "rotation", index=n)]
        return []

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        plan = [(int(e["source"]) - 1, int(e.get("rotate", 0))) for e in params["pages"]]
        with ctx.open_pdf(ctx.inputs[0]) as src:
            self.warn_shared(ctx, [src])
            if is_tagged(src):
                ctx.warn("TAGS_DROPPED")
            sources = {i for i, _ in plan}
            duplicated = len(sources) != len(plan)
            deleted = len(sources) != len(src.pages)
            if field_names(src) and (duplicated or deleted):
                # A widget on a duplicated page, or a field whose page is gone,
                # breaks the form; flatten instead (shared rule 13.2).
                flatten_forms(src)
                ctx.warn("FORMS_FLATTENED")

            ctx.stage("organize", 1, 2)
            dst = pikepdf.new()
            for done, (i, rotate) in enumerate(plan, start=1):
                ctx.check_cancelled()
                dst.pages.append(src.pages[i])
                if rotate:
                    dst.pages[-1].rotate(rotate, relative=True)
                ctx.progress("organize", done, len(plan))
            for page in dst.pages:
                if Name.StructParents in page.obj:
                    del page.obj[Name.StructParents]
            if Name.AcroForm in src.Root and not (duplicated or deleted):
                dst.Root.AcroForm = copy_into(dst, src, src.Root.AcroForm)

            page_map: dict[int, int] = {}
            for j, (i, _) in enumerate(plan):
                page_map.setdefault(i, j)
            with dst.open_outline() as outline:
                copy_outline(src, page_map, outline.root)
            every = {i: i for i in range(len(src.pages))}
            if copy_outline(src, every, []) > 0:
                ctx.warn("OUTLINE_DROPPED")
            for key, value in src.docinfo.items():
                if isinstance(value, pikepdf.String) and key != "/Producer":
                    dst.docinfo[key] = pikepdf.String(str(value))

            ctx.stage("write", 2, 2)
            out = ctx.output_path("organized.pdf")
            pages = save_pdf(ctx, dst, out)
            return [OutputFile(out, pages=pages)]


if __name__ == "__main__":
    run_main(PdfOrganizeAdapter)
