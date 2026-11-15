"""pdf-redact: remove content under boxes and every match of search terms (spec section 13.8).

**A false "redacted" is much worse than a failed job.** So the output is
only offered after ``verify.py`` passes it in a fresh process that sees
nothing but the output file and a list of what to check. When verification
fails, the output is deleted and the job fails with
``E_REDACTION_VERIFICATION_FAILED``; with ``rasterizeFallback`` the affected
pages are turned into images with the redactions burned in, and verified
again.

Pipeline:
1. Boxes arrive normalized in *displayed* orientation (after /Rotate and
   the CropBox). Term matches are found and converted to the same form, so
   every region is described one way, independent of any engine.
2. PyMuPDF applies redactions per page: text, image pixels, covered line art.
3. Document-level content is stripped (spec 13.8 "Strip step").
4. pikepdf does a full rewrite: no incremental save, unreferenced objects
   dropped, one cross-reference section.
5. ``verify.py`` runs V1 to V9 in a new process.

This file and ``verify.py`` are each self-contained: workers run with
``python -I``, so neither can import the other.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import AdapterError, JobContext, OutputFile, PageWords, run_main
from converter_sdk.engines import helper_env
from converter_sdk.pdf import (
    OpenedInput,
    PdfAdapter,
    is_tagged,
    param_issue,
    remove_hidden_layers,
    save_pdf,
)
from converter_sdk.protocol import ValidationIssue, WordBox
from pikepdf import Name

MB = 1024 * 1024
VERIFY = Path(__file__).resolve().with_name("verify.py")
VERIFY_TIMEOUT_S = 600
RASTER_DPI = 200
PRODUCER = "Local File Conversion Suite"
FILLS = {"black": (0.0, 0.0, 0.0), "white": (1.0, 1.0, 1.0)}
MIN_CAPTURE = 3
"""Captured text shorter than this (after normalization) isn't searched for elsewhere."""


def _fitz():
    try:
        import pymupdf  # noqa: PLC0415 - engine imported on demand
    except ImportError:
        raise AdapterError("E_INSTALL_CORRUPT", {"missing": "pymupdf"}) from None
    return pymupdf


# ------------------------------------------------------------------ text


def _fold(text: str) -> str:
    return unicodedata.normalize("NFKC", text).casefold()


def normalize(text: str) -> str:
    """Case-folded, hyphenated line breaks joined, whitespace collapsed."""
    text = _fold(text).replace("­", "")
    text = text.replace("-\n", "")
    return " ".join(text.split())


@dataclass
class Char:
    text: str
    rect: Any  # pymupdf.Rect in unrotated page space, or None for a line break
    line: int


def page_chars(page: Any) -> list[Char]:
    """Every character with its box, plus a ``\\n`` marker between lines.
    Includes invisible text (render mode 3), which MuPDF extracts like any other."""
    fitz = _fitz()
    flags = fitz.TEXT_PRESERVE_WHITESPACE | fitz.TEXT_PRESERVE_LIGATURES
    data = page.get_text("rawdict", flags=flags)
    chars: list[Char] = []
    line_no = 0
    for block in data.get("blocks", []):
        for line in block.get("lines", []):
            line_no += 1
            for span in line.get("spans", []):
                for ch in span.get("chars", []):
                    chars.append(Char(ch["c"], fitz.Rect(ch["bbox"]), line_no))
            chars.append(Char("\n", None, line_no))
    return chars


def _stream(chars: list[Char], keep_spaces: bool) -> tuple[str, list[Char | None]]:
    """A normalized string plus, for each of its characters, the source char."""
    out: list[str] = []
    owners: list[Char | None] = []
    for i, ch in enumerate(chars):
        if ch.text == "\n":
            if out and out[-1] == "-":
                # Hyphenated line break: drop the hyphen, join the word.
                out.pop()
                owners.pop()
                continue
            ch_text = " "
        else:
            ch_text = _fold(ch.text).replace("­", "")
        for c in ch_text:
            if c.isspace():
                if not keep_spaces or (out and out[-1] == " ") or not out:
                    continue
                out.append(" ")
                owners.append(None)
            else:
                out.append(c)
                owners.append(chars[i] if ch.text != "\n" else None)
    return "".join(out), owners


def find_terms(chars: list[Char], terms: list[str]) -> list[list[Char]]:
    """Matches as lists of chars. Searches with whitespace collapsed and again
    with whitespace removed, so text split across operators still matches."""
    found: list[list[Char]] = []
    for keep_spaces in (True, False):
        text, owners = _stream(chars, keep_spaces)
        for term in terms:
            needle = normalize(term)
            if not keep_spaces:
                needle = needle.replace(" ", "")
            if not needle:
                continue
            start = text.find(needle)
            while start != -1:
                hit = [o for o in owners[start : start + len(needle)] if o is not None]
                if hit:
                    found.append(hit)
                start = text.find(needle, start + 1)
    return found


def union_by_line(chars: list[Char]) -> list[Any]:
    rects: dict[int, Any] = {}
    for ch in chars:
        rects[ch.line] = ch.rect if ch.line not in rects else rects[ch.line] | ch.rect
    return list(rects.values())


# -------------------------------------------------------------- geometry


@dataclass(frozen=True)
class Region:
    """Normalized 0..1, displayed orientation, origin top-left."""

    page: int  # 0-based
    x0: float
    y0: float
    x1: float
    y1: float

    def as_json(self) -> dict[str, float | int]:
        return {"page": self.page + 1, "x0": self.x0, "y0": self.y0, "x1": self.x1, "y1": self.y1}


def to_page_rect(page: Any, r: Region) -> Any:
    """Displayed-normalized → unrotated page space, as MuPDF expects for redaction."""
    fitz = _fitz()
    shown = page.rect  # displayed size, after /Rotate
    rect = fitz.Rect(
        r.x0 * shown.width, r.y0 * shown.height, r.x1 * shown.width, r.y1 * shown.height
    )
    return (rect * page.derotation_matrix).normalize()


def to_region(page: Any, number: int, rect: Any, pad: float = 0.5) -> Region:
    """Unrotated page space → displayed-normalized, padded slightly so glyph
    edges are covered."""
    shown = page.rect
    d = (rect * page.rotation_matrix).normalize()
    w, h = shown.width, shown.height
    return Region(
        number,
        max(0.0, (d.x0 - pad) / w),
        max(0.0, (d.y0 - pad) / h),
        min(1.0, (d.x1 + pad) / w),
        min(1.0, (d.y1 + pad) / h),
    )


# --------------------------------------------------------------- the plan


@dataclass
class Plan:
    regions: dict[int, list[Region]] = field(default_factory=dict)
    captured: list[str] = field(default_factory=list)
    no_text_pages: list[int] = field(default_factory=list)

    def add(self, region: Region) -> None:
        self.regions.setdefault(region.page, []).append(region)

    def all(self) -> list[Region]:
        return [r for rs in self.regions.values() for r in rs]


def plan_redactions(ctx: JobContext, doc: Any, params: dict[str, Any]) -> Plan:
    plan = Plan()
    terms = [t for t in params.get("terms") or [] if normalize(t)]
    boxes_by_page: dict[int, list[dict[str, Any]]] = {}
    for b in params.get("boxes") or []:
        boxes_by_page.setdefault(int(b["page"]) - 1, []).append(b)
    total = len(doc)
    for number in range(total):
        ctx.check_cancelled()
        page = doc[number]
        chars = page_chars(page) if (terms or number in boxes_by_page) else []
        for b in boxes_by_page.get(number, []):
            region = Region(
                number,
                min(b["x0"], b["x1"]),
                min(b["y0"], b["y1"]),
                max(b["x0"], b["x1"]),
                max(b["y0"], b["y1"]),
            )
            plan.add(region)
            rect = to_page_rect(page, region)
            under = "".join(
                c.text for c in chars if c.rect is not None and _centre_in(c.rect, rect)
            )
            for piece in normalize(under).split(" "):
                if len(piece) >= MIN_CAPTURE:
                    plan.captured.append(piece)
        if terms:
            if not any(c.rect is not None and not c.text.isspace() for c in chars):
                if page.get_images(full=False):
                    plan.no_text_pages.append(number + 1)
            for match in find_terms(chars, terms):
                for rect in union_by_line(match):
                    plan.add(to_region(page, number, rect))
        ctx.progress("plan", number + 1, total)
    plan.captured = sorted(set(plan.captured))
    return plan


def _centre_in(char_rect: Any, rect: Any) -> bool:
    cx = (char_rect.x0 + char_rect.x1) / 2
    cy = (char_rect.y0 + char_rect.y1) / 2
    return rect.x0 <= cx <= rect.x1 and rect.y0 <= cy <= rect.y1


# ------------------------------------------------------------ redaction


def apply(doc: Any, plan: Plan, ctx: JobContext) -> None:
    fitz = _fitz()
    for number, regions in sorted(plan.regions.items()):
        ctx.check_cancelled()
        page = doc[number]
        for region in regions:
            # No fill from MuPDF: it misplaces drawings on rotated pages with an offset
            # CropBox. ``draw_fills`` paints them in PDF space instead.
            page.add_redact_annot(to_page_rect(page, region), fill=False, cross_out=False)
        page.apply_redactions(
            images=fitz.PDF_REDACT_IMAGE_PIXELS,
            graphics=fitz.PDF_REDACT_LINE_ART_REMOVE_IF_COVERED,
            text=fitz.PDF_REDACT_TEXT_REMOVE,
        )


def strip_with_mupdf(doc: Any, terms: list[str]) -> None:
    """Document-level content that MuPDF handles best."""
    doc.scrub(
        attached_files=True,
        clean_pages=True,
        embedded_files=True,
        hidden_text=False,
        javascript=True,
        metadata=True,
        redactions=False,
        remove_links=True,
        reset_fields=True,
        reset_responses=True,
        thumbnails=True,
        xml_metadata=True,
    )
    for page in doc:
        for widget in list(page.widgets()):
            page.delete_widget(widget)
        for annot in list(page.annots()):
            page.delete_annot(annot)
    needles = [normalize(t) for t in terms]
    toc = doc.get_toc(simple=True)
    kept: list[list[Any]] = []
    for level, title, target in toc:
        if any(n and n in normalize(title) for n in needles):
            continue
        level = min(level, (kept[-1][0] + 1) if kept else 1)
        kept.append([level, title, target])
    if kept != toc:
        doc.set_toc(kept)


def strip_with_pikepdf(pdf: pikepdf.Pdf) -> None:
    """Everything else in the strip step, on the structure itself."""
    root = pdf.Root
    for key in (Name.Metadata, Name.AcroForm, Name.OpenAction, Name.AA, Name.PieceInfo):
        if key in root:
            del root[key]
    names = root.get(Name.Names)
    if isinstance(names, pikepdf.Dictionary):
        for key in list(names.keys()):
            if key != "/Dests":
                del names[key]
    for obj in pdf.objects:
        if isinstance(obj, (pikepdf.Dictionary, pikepdf.Stream)):
            target = obj.stream_dict if isinstance(obj, pikepdf.Stream) else obj
            for key in ("/Metadata", "/PieceInfo", "/Thumb", "/AA"):
                if key in target:
                    del target[key]
    for page in pdf.pages:
        if Name.Annots in page.obj:
            del page.obj[Name.Annots]
    for key in list(pdf.docinfo.keys()):
        del pdf.docinfo[key]
    pdf.docinfo[Name.Producer] = PRODUCER


def _inherited(page: pikepdf.Page, key: Name) -> Any:
    node = page.obj
    for _ in range(64):
        if key in node:
            return node[key]
        parent = node.get(Name.Parent)
        if not isinstance(parent, pikepdf.Dictionary):
            return None
        node = parent
    return None


def displayed_to_user(page: pikepdf.Page, r: Region) -> tuple[float, float, float, float]:
    """Displayed-normalized region → PDF user space, from the CropBox and /Rotate."""
    box = _inherited(page, Name.CropBox) or _inherited(page, Name.MediaBox)
    xs = sorted((float(box[0]), float(box[2])))
    ys = sorted((float(box[1]), float(box[3])))
    left, bottom, w, h = xs[0], ys[0], xs[1] - xs[0], ys[1] - ys[0]
    rotate = int(_inherited(page, Name.Rotate) or 0) % 360
    dw, dh = (h, w) if rotate in (90, 270) else (w, h)
    points = []
    for nx, ny in ((r.x0, r.y0), (r.x1, r.y1)):
        u, v = nx * dw, dh - ny * dh
        if rotate == 90:
            points.append((left + w - v, bottom + u))
        elif rotate == 180:
            points.append((left + w - u, bottom + h - v))
        elif rotate == 270:
            points.append((left + v, bottom + h - u))
        else:
            points.append((left + u, bottom + v))
    (ax, ay), (bx, by) = points
    return min(ax, bx), min(ay, by), max(ax, bx), max(ay, by)


def draw_fills(
    pdf: pikepdf.Pdf, regions: dict[int, list[Region]], fill: tuple[float, float, float]
) -> None:
    """Paint each region on top of everything else on its page."""
    r, g, b = fill
    for number, rs in regions.items():
        page = pdf.pages[number]
        ops = [f"q {r:g} {g:g} {b:g} rg"]
        for region in rs:
            x0, y0, x1, y1 = displayed_to_user(page, region)
            ops.append(f"{x0:.3f} {y0:.3f} {x1 - x0:.3f} {y1 - y0:.3f} re f")
        ops.append("Q")
        page.contents_add(pdf.make_stream(b"q\n"), prepend=True)
        page.contents_add(pdf.make_stream(("Q\n" + "\n".join(ops) + "\n").encode("ascii")))


def rasterize(doc: Any, pages: set[int]) -> Any:
    """A new document where ``pages`` are images of the redacted rendering.
    Other pages are copied unchanged. Rotation is baked into the image."""
    fitz = _fitz()
    out = fitz.open()
    scale = fitz.Matrix(RASTER_DPI / 72, RASTER_DPI / 72)
    for number in range(len(doc)):
        page = doc[number]
        if number in pages:
            pix = page.get_pixmap(matrix=scale, annots=False, alpha=False)
            new = out.new_page(width=page.rect.width, height=page.rect.height)
            new.insert_image(new.rect, pixmap=pix)
        else:
            out.insert_pdf(doc, from_page=number, to_page=number, annots=False, links=False)
    return out


# ------------------------------------------------------------ verifying


def run_verifier(ctx: JobContext, output: Path, spec: dict[str, Any]) -> dict[str, Any]:
    """Run ``verify.py`` in a fresh process. It sees only the output file
    (job-relative) and what to check, on stdin."""
    env = helper_env()
    request = json.dumps({**spec, "output": output.relative_to(ctx.job_dir.resolve()).as_posix()})
    proc = subprocess.Popen(  # noqa: S603 - fixed argv: our interpreter and our verifier
        [sys.executable, "-I", str(VERIFY)],
        cwd=ctx.job_dir,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        # Read and dropped, not DEVNULL: a Windows AppContainer is denied
        # the null device. It can hold document text, so it's never kept.
        stderr=subprocess.PIPE,
    )
    if proc.stdin is None or proc.stdout is None or proc.stderr is None:  # pragma: no cover
        raise AdapterError("E_INTERNAL", {"reason": "verifierPipes"})
    err = proc.stderr

    def drop_stderr() -> None:
        while err.read(65536):
            pass

    threading.Thread(target=drop_stderr, daemon=True).start()
    proc.stdin.write(request.encode("utf-8"))
    proc.stdin.close()
    deadline = time.monotonic() + VERIFY_TIMEOUT_S
    while proc.poll() is None:
        if ctx.cancelled or time.monotonic() > deadline:
            proc.kill()
            proc.wait(5)
            ctx.check_cancelled()
            raise AdapterError("E_TIMEOUT", {"stage": "verify"})
        time.sleep(0.05)
    raw = proc.stdout.read()
    try:
        report = json.loads(raw)
        if not isinstance(report.get("checks"), list):
            raise ValueError("no checks")
    except (ValueError, AttributeError):
        # A verifier that can't report is a failed verification, never a pass.
        return {"checks": [{"id": "V0", "passed": False, "reason": "verifierFailed"}]}
    return report


def failed(report: dict[str, Any]) -> list[dict[str, Any]]:
    return [c for c in report["checks"] if not c.get("passed")]


# --------------------------------------------------------------- adapter


class PdfRedactAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-redact"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"
    base_ram_bytes: ClassVar[int] = 256 * MB  # pikepdf, MuPDF, pdfium and page renders
    ram_per_input_byte: ClassVar[float] = 1.0
    disk_factor: ClassVar[float] = 4.0

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        if len(inputs) != 1:
            return [param_issue("inputs", "exactlyOne")]
        total = inputs[0].pages
        boxes = params.get("boxes") or []
        terms = params.get("terms") or []
        if not boxes and not terms:
            return [param_issue("boxes", "nothingToRedact")]
        for n, b in enumerate(boxes):
            try:
                page = int(b["page"])
                xs = (float(b["x0"]), float(b["x1"]))
                ys = (float(b["y0"]), float(b["y1"]))
            except (KeyError, TypeError, ValueError):
                return [param_issue("boxes", "invalid", index=n)]
            if not 1 <= page <= total:
                return [param_issue("boxes", "pageOutOfBounds", index=n, pages=total)]
            if not all(0 <= v <= 1 for v in (*xs, *ys)) or xs[0] == xs[1] or ys[0] == ys[1]:
                return [param_issue("boxes", "empty", index=n)]
        for n, t in enumerate(terms):
            if not isinstance(t, str) or len(normalize(t)) < 2:
                return [param_issue("terms", "tooShort", index=n)]
        if params.get("fill", "black") not in FILLS:
            return [param_issue("fill", "invalid")]
        return []

    def page_text(self, ctx: JobContext, input: Path, page: int) -> PageWords:
        """Word boxes in displayed orientation, for the redaction canvas."""
        fitz = _fitz()
        doc = self._open(ctx, input)
        try:
            if not 1 <= page <= len(doc):
                raise AdapterError("E_INTERNAL", {"reason": "pageOutOfRange"}, retryable=False)
            p = doc[page - 1]
            words = []
            for x0, y0, x1, y1, text, *_ in p.get_text("words")[:20000]:
                d = (fitz.Rect(x0, y0, x1, y1) * p.rotation_matrix).normalize()
                words.append(WordBox(text=text[:1000], x0=d.x0, y0=d.y0, x1=d.x1, y1=d.y1))
            return PageWords(width=p.rect.width, height=p.rect.height, words=words)
        finally:
            doc.close()

    @staticmethod
    def _open(ctx: JobContext, path: Path) -> Any:
        fitz = _fitz()
        try:
            doc = fitz.open(path, filetype="pdf")
        except (fitz.FileDataError, RuntimeError):
            raise AdapterError("E_INPUT_CORRUPT") from None
        if doc.needs_pass:
            if ctx.secrets is None or ctx.secrets.password is None:
                doc.close()
                raise AdapterError("E_PASSWORD_REQUIRED")
            if not doc.authenticate(ctx.secrets.password.get_secret_value()):
                doc.close()
                raise AdapterError("E_PASSWORD_WRONG")
        return doc

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        fill = FILLS[params.get("fill", "black")]
        terms = [t for t in params.get("terms") or [] if normalize(t)]
        source = ctx.inputs[0]

        with ctx.open_pdf(source) as original:
            self.warn_shared(ctx, [original])
            if is_tagged(original):
                ctx.warn("TAGS_DROPPED")
            if Name.AcroForm in original.Root:
                ctx.warn("FORMS_FLATTENED", {"removed": True})
            expected_pages = len(original.pages)

        # Pre-pass with pikepdf: decrypt and remove hidden layers, so MuPDF
        # plans and redacts exactly what a viewer can show.
        ctx.stage("plan", 1, 5)
        prepared = ctx.temp_path(".pdf")
        with ctx.open_pdf(source) as pdf:
            remove_hidden_layers(pdf)
            pdf.save(prepared, encryption=False, fix_metadata_version=False)
        doc = self._open(ctx, prepared)
        try:
            plan = plan_redactions(ctx, doc, params)
            if plan.no_text_pages:
                ctx.warn("NO_TEXT_LAYER", {"pages": plan.no_text_pages[:100]})

            ctx.stage("redact", 2, 5)
            apply(doc, plan, ctx)
            strip_with_mupdf(doc, terms)
            output = self._write(ctx, doc, plan.regions, fill)

            spec = {
                "expectedPages": expected_pages,
                "fill": [round(c * 255) for c in fill],
                "terms": terms,
                "captured": plan.captured,
                "regions": [r.as_json() for r in plan.all()],
            }
            ctx.stage("verify", 4, 5)
            report = run_verifier(ctx, output, spec)
            failures = failed(report)
            if failures and params.get("rasterizeFallback") is True:
                output.unlink(missing_ok=True)
                ctx.stage("rasterize", 4, 5)
                affected = self._affected(plan, failures, len(doc))
                flat = rasterize(doc, affected)
                try:
                    strip_with_mupdf(flat, terms)
                    output = self._write(ctx, flat, plan.regions, fill)
                finally:
                    flat.close()
                report = run_verifier(ctx, output, spec)
                failures = failed(report)
            if failures:
                output.unlink(missing_ok=True)
                raise AdapterError(
                    "E_REDACTION_VERIFICATION_FAILED",
                    {
                        "failed": sorted({c["id"] for c in failures}),
                        "fallbackAvailable": params.get("rasterizeFallback") is not True,
                    },
                    retryable=False,
                )
        finally:
            doc.close()

        elsewhere = report.get("elsewhere") or {}
        if elsewhere.get("count"):
            ctx.warn(
                "REDACTED_TEXT_REMAINS_ELSEWHERE",
                {"count": int(elsewhere["count"]), "pages": list(elsewhere.get("pages", []))[:100]},
            )
        for n, check in enumerate(report["checks"], start=1):
            ctx.stage(f"verified-{check['id']}", n, len(report["checks"]))
        return [OutputFile(output, pages=expected_pages)]

    def _write(
        self,
        ctx: JobContext,
        doc: Any,
        regions: dict[int, list[Region]],
        fill: tuple[float, float, float],
    ) -> Path:
        """MuPDF writes a clean intermediate; pikepdf strips and does the full rewrite."""
        fitz = _fitz()
        ctx.stage("write", 3, 5)
        stage1 = ctx.temp_path(".pdf")
        doc.save(
            stage1,
            garbage=4,
            deflate=True,
            clean=True,
            incremental=False,
            encryption=fitz.PDF_ENCRYPT_NONE,
        )
        out = ctx.output_path("redacted.pdf")
        with pikepdf.open(stage1) as pdf:
            strip_with_pikepdf(pdf)
            draw_fills(pdf, regions, fill)
            save_pdf(ctx, pdf, out)
        stage1.unlink(missing_ok=True)
        return out

    @staticmethod
    def _affected(plan: Plan, failures: list[dict[str, Any]], total: int) -> set[int]:
        pages = set(plan.regions)
        for check in failures:
            listed = check.get("pages")
            if listed:
                pages |= {int(p) - 1 for p in listed}
            else:
                # A document-level failure (raw streams, structure): every page.
                return set(range(total))
        return pages


if __name__ == "__main__":
    run_main(PdfRedactAdapter)
