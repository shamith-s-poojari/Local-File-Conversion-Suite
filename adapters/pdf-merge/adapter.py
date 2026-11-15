"""pdf-merge: concatenate PDFs in order (spec section 13.3)."""

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
from pikepdf import Array, Dictionary, Name

BOOKMARK_MODES = ("perFile", "keep", "none")


class PdfMergeAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-merge"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        issues = []
        if params.get("bookmarks", "perFile") not in BOOKMARK_MODES:
            issues.append(param_issue("bookmarks", "invalid"))
        source = params.get("metadataFrom", 0)
        if not isinstance(source, int) or not -1 <= source < len(inputs):
            issues.append(param_issue("metadataFrom", "outOfBounds"))
        titles = params.get("fileTitles")
        if titles is not None and (not isinstance(titles, list) or len(titles) > len(inputs)):
            issues.append(param_issue("fileTitles", "tooMany"))
        if len(inputs) < 2:
            issues.append(param_issue("inputs", "tooFew", min=2))
        if sum(i.pages for i in inputs) > self.max_pages * 10:
            issues.append(ValidationIssue(code="E_LIMIT_EXCEEDED", detail={"limit": "pages"}))
        return issues

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        mode = params.get("bookmarks", "perFile")
        metadata_from = params.get("metadataFrom", 0)
        titles = list(params.get("fileTitles") or [])

        sources: list[pikepdf.Pdf] = []
        try:
            ctx.stage("open", 1, 3)
            sources = self.open_inputs(ctx)
            self.warn_shared(ctx, sources)
            if any(is_tagged(s) for s in sources):
                ctx.warn("TAGS_DROPPED")

            # Field names must be unique across the result. A later file whose
            # names clash is flattened, keeping its look but not its fields.
            seen: set[str] = set()
            keep_fields: list[pikepdf.Pdf] = []
            flattened = 0
            for src in sources:
                names = field_names(src)
                if not names:
                    continue
                if names & seen:
                    flatten_forms(src)
                    flattened += 1
                else:
                    seen |= names
                    keep_fields.append(src)
            if flattened:
                ctx.warn("FORMS_FLATTENED", {"files": flattened})

            ctx.stage("merge", 2, 3)
            total = sum(len(s.pages) for s in sources)
            dst = pikepdf.new()
            starts: list[int] = []
            done = 0
            for src in sources:
                starts.append(len(dst.pages))
                for page in src.pages:
                    ctx.check_cancelled()
                    dst.pages.append(page)
                    done += 1
                    ctx.progress("merge", done, total)
            for page in dst.pages:
                if Name.StructParents in page.obj:
                    del page.obj[Name.StructParents]

            self._merge_fields(dst, keep_fields)
            dropped = self._bookmarks(dst, sources, starts, mode, titles)
            if dropped:
                ctx.warn("OUTLINE_DROPPED", {"items": dropped})
            if 0 <= metadata_from < len(sources):
                for key, value in sources[metadata_from].docinfo.items():
                    if isinstance(value, pikepdf.String) and key != "/Producer":
                        dst.docinfo[key] = pikepdf.String(str(value))

            ctx.stage("write", 3, 3)
            out = ctx.output_path("merged.pdf")
            pages = save_pdf(ctx, dst, out)
            return [OutputFile(out, pages=pages)]
        finally:
            for src in sources:
                src.close()

    @staticmethod
    def _merge_fields(dst: pikepdf.Pdf, sources: list[pikepdf.Pdf]) -> None:
        """Rebuild /AcroForm from the inputs whose fields survived.

        ``copy_foreign`` reuses the copies already made when pages were
        appended, so field dictionaries stay the same objects as the widgets.
        """
        if not sources:
            return
        fields = Array()
        need_appearances = False
        for src in sources:
            form = src.Root.AcroForm
            for field in form.get(Name.Fields, Array()):
                fields.append(copy_into(dst, src, field))
            need_appearances |= bool(form.get(Name.NeedAppearances, False))
        first = sources[0].Root.AcroForm
        form = Dictionary(Fields=fields)
        if Name.DA in first:
            form.DA = pikepdf.String(str(first.DA))
        if Name.DR in first:
            form.DR = copy_into(dst, sources[0], first.DR)
        if need_appearances:
            form.NeedAppearances = True
        dst.Root.AcroForm = dst.make_indirect(form)

    @staticmethod
    def _bookmarks(
        dst: pikepdf.Pdf,
        sources: list[pikepdf.Pdf],
        starts: list[int],
        mode: str,
        titles: list[str],
    ) -> int:
        if mode == "none":
            return 0
        dropped = 0
        with dst.open_outline() as outline:
            for n, (src, start) in enumerate(zip(sources, starts, strict=True)):
                page_map = {i: start + i for i in range(len(src.pages))}
                if mode == "perFile":
                    title = titles[n] if n < len(titles) and titles[n] else f"Document {n + 1}"
                    item = pikepdf.OutlineItem(title, start)
                    dropped += copy_outline(src, page_map, item.children)
                    outline.root.append(item)
                else:
                    dropped += copy_outline(src, page_map, outline.root)
        return dropped


if __name__ == "__main__":
    run_main(PdfMergeAdapter)
