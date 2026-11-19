"""Redaction verifier (spec section 13.8, checks V1 to V10).

Runs in a **fresh process** started by ``adapter.py``. It sees only the
output file and a JSON list of what to check, read from stdin, and prints a
JSON report on stdout. It shares no code with the adapter, and uses three
engines (PyMuPDF, pdfium, pikepdf) so that one engine's blind spot can't
pass silently. In particular, V2 maps the user's boxes to PDF space with
its own arithmetic, so a coordinate bug in the adapter shows up as a
failure instead of a false pass.

Request::

    {"output": "out/0.pdf", "expectedPages": 3, "fill": [0, 0, 0],
     "terms": [...], "captured": [...],
     "regions": [{"page": 1, "x0": 0.1, "y0": 0.1, "x1": 0.3, "y1": 0.2}]}

Regions are normalized 0..1 in displayed orientation, origin top-left.
The report never contains document text, only check IDs, pages and counts.
"""

from __future__ import annotations

import io
import json
import sys
import unicodedata
from pathlib import Path
from typing import Any

import pikepdf
import pymupdf
import pypdfium2 as pdfium

V6_DPI = 150
V6_INSET_PX = 2
V6_TOLERANCE = 3
PRODUCER_ONLY = {"/Producer"}


def normalize(text: str) -> str:
    text = unicodedata.normalize("NFKC", text).casefold().replace("­", "")
    text = text.replace("-\n", "")
    return " ".join(text.split())


def variants(text: str) -> tuple[str, str]:
    n = normalize(text)
    return n, n.replace(" ", "")


def contains_any(haystack: str, needles: list[str]) -> bool:
    spaced, tight = variants(haystack)
    for needle in needles:
        n_spaced, n_tight = variants(needle)
        if n_spaced and (n_spaced in spaced or n_tight in tight):
            return True
    return False


def overlaps_core(box: tuple[float, float, float, float], region: tuple[float, ...]) -> bool:
    """True if the central half of ``box`` intersects ``region`` (both x0, y0, x1, y1)."""
    x0, y0, x1, y1 = box
    qx, qy = (x1 - x0) / 4, (y1 - y0) / 4
    cx0, cy0, cx1, cy1 = x0 + qx, y0 + qy, x1 - qx, y1 - qy
    rx0, ry0, rx1, ry1 = region
    return cx0 < rx1 and cx1 > rx0 and cy0 < ry1 and cy1 > ry0


def by_page(regions: list[dict[str, Any]]) -> dict[int, list[dict[str, Any]]]:
    out: dict[int, list[dict[str, Any]]] = {}
    for r in regions:
        out.setdefault(int(r["page"]) - 1, []).append(r)
    return out


# --------------------------------------------------------------- checks


def v1_mupdf_regions(doc: Any, regions: dict[int, list[dict[str, Any]]]) -> list[int]:
    bad = []
    for number, rs in regions.items():
        page = doc[number]
        shown = page.rect
        rects = []
        for r in rs:
            d = pymupdf.Rect(
                r["x0"] * shown.width,
                r["y0"] * shown.height,
                r["x1"] * shown.width,
                r["y1"] * shown.height,
            )
            rects.append(tuple((d * page.derotation_matrix).normalize()))
        data = page.get_text("rawdict")
        hit = False
        for block in data.get("blocks", []):
            for line in block.get("lines", []):
                for span in line.get("spans", []):
                    for ch in span.get("chars", []):
                        if ch["c"].isspace():
                            continue
                        if any(overlaps_core(tuple(ch["bbox"]), rect) for rect in rects):
                            hit = True
        if hit:
            bad.append(number + 1)
    return bad


def displayed_to_user(page: Any, r: dict[str, Any]) -> tuple[float, float, float, float]:
    """Independent of MuPDF: displayed-normalized box → PDF user space, from
    the CropBox and /Rotate as pdfium reports them."""
    left, bottom, right, top = page.get_cropbox()
    w, h = right - left, top - bottom
    rotate = page.get_rotation() % 360
    dw, dh = (h, w) if rotate in (90, 270) else (w, h)
    points = []
    for nx, ny in ((r["x0"], r["y0"]), (r["x1"], r["y1"])):
        u = nx * dw
        v = dh - ny * dh  # displayed, origin bottom-left
        if rotate == 90:
            x, y = left + w - v, bottom + u
        elif rotate == 180:
            x, y = left + w - u, bottom + h - v
        elif rotate == 270:
            x, y = left + v, bottom + h - u
        else:
            x, y = left + u, bottom + v
        points.append((x, y))
    (ax, ay), (bx, by) = points
    return min(ax, bx), min(ay, by), max(ax, bx), max(ay, by)


def v2_pdfium_regions(pdf: Any, regions: dict[int, list[dict[str, Any]]]) -> list[int]:
    bad = []
    for number, rs in regions.items():
        page = pdf[number]
        rects = [displayed_to_user(page, r) for r in rs]
        text = page.get_textpage()
        hit = False
        for i in range(text.count_chars()):
            ch = text.get_text_range(i, 1)
            if not ch or ch.isspace():
                continue
            left, bottom, right, top = text.get_charbox(i, loose=False)
            if right <= left or top <= bottom:
                continue
            if any(overlaps_core((left, bottom, right, top), rect) for rect in rects):
                hit = True
                break
        text.close()
        if hit:
            bad.append(number + 1)
    return bad


def page_texts_mupdf(doc: Any) -> list[str]:
    return [p.get_text() for p in doc]


def page_texts_pdfium(pdf: Any) -> list[str]:
    out = []
    for page in pdf:
        tp = page.get_textpage()
        out.append(tp.get_text_bounded())
        tp.close()
    return out


def v3_terms(texts_a: list[str], texts_b: list[str], terms: list[str]) -> list[int]:
    if not terms:
        return []
    return [
        n + 1
        for n, (a, b) in enumerate(zip(texts_a, texts_b, strict=True))
        if contains_any(a, terms) or contains_any(b, terms)
    ]


def v4_raw(path: Path, terms: list[str]) -> bool:
    """True if any term survives in any decoded stream or string.

    Not QDF mode: qpdf's QDF output adds comments such as "%% Page 2",
    which would match terms that aren't in the document."""
    if not terms:
        return False
    with pikepdf.open(path) as pdf:
        buf = io.BytesIO()
        pdf.save(
            buf,
            stream_decode_level=pikepdf.StreamDecodeLevel.all,
            object_stream_mode=pikepdf.ObjectStreamMode.disable,
            compress_streams=False,
        )
    data = buf.getvalue().lower()
    for term in terms:
        for enc in ("latin-1", "utf-8", "utf-16-be"):
            raw = term.casefold().encode(enc, errors="ignore")
            if len(raw) < 2:
                continue
            if raw in data or raw.hex().encode() in data:
                return True
    return False


def v5_structure(path: Path) -> list[str]:
    reasons = []
    with pikepdf.open(path) as pdf:
        if set(pdf.docinfo.keys()) - PRODUCER_ONLY:
            reasons.append("info")
        root = pdf.Root
        if "/Metadata" in root:
            reasons.append("xmp")
        if "/AcroForm" in root:
            reasons.append("form")
        if "/OpenAction" in root or "/AA" in root:
            reasons.append("actions")
        names = root.get("/Names")
        if isinstance(names, pikepdf.Dictionary):
            if "/EmbeddedFiles" in names:
                reasons.append("attachments")
            if "/JavaScript" in names:
                reasons.append("javascript")
        props = root.get("/OCProperties")
        if isinstance(props, pikepdf.Dictionary):
            d = props.get("/D")
            off = d.get("/OFF") if isinstance(d, pikepdf.Dictionary) else None
            base_off = isinstance(d, pikepdf.Dictionary) and d.get("/BaseState") == "/OFF"
            if (isinstance(off, pikepdf.Array) and len(off) > 0) or base_off:
                reasons.append("hiddenLayers")
            # Auto-state arrays switch layers on or off by viewer event.
            configs = props.get("/Configs")
            every = [d] + (list(configs) if isinstance(configs, pikepdf.Array) else [])
            if any(isinstance(c, pikepdf.Dictionary) and "/AS" in c for c in every):
                reasons.append("layerStates")
        for key in ("/StructTreeRoot", "/MarkInfo"):
            if key in root:
                reasons.append("tags")
        if "/Threads" in root:
            reasons.append("threads")
        for page in pdf.pages:
            annots = page.obj.get("/Annots")
            if isinstance(annots, pikepdf.Array) and len(annots) > 0:
                reasons.append("annotations")
                break
        for obj in pdf.objects:
            if isinstance(obj, pikepdf.Stream):
                obj = obj.stream_dict
            if not isinstance(obj, pikepdf.Dictionary):
                continue
            if "/Metadata" in obj:
                reasons.append("xmp")
            if "/Thumb" in obj:
                reasons.append("thumbnails")
            if "/PieceInfo" in obj:
                reasons.append("pieceInfo")
            if obj.get("/S") == "/JavaScript" or "/JS" in obj:
                reasons.append("javascript")
            if obj.get("/Type") == "/Filespec" and "/EF" in obj:
                reasons.append("attachments")
    return sorted(set(reasons))


TEXT_KEYS = ("/ActualText", "/Alt", "/E", "/TU")
NO_FOLLOW_TYPES = {"/Page", "/Pages", "/Catalog"}


def pdf_string(obj: Any) -> str | None:
    if not isinstance(obj, pikepdf.String):
        return None
    try:
        return str(obj)
    except (UnicodeDecodeError, ValueError):
        return bytes(obj).decode("latin-1")


def strings_below(start: Any, limit: int = 200_000) -> list[str]:
    """Every string reachable from ``start`` without entering a page (outline
    items, page labels and article beads all point at pages)."""
    out: list[str] = []
    seen: set[tuple[int, int]] = set()
    stack = [start]
    while stack and limit > 0:
        limit -= 1
        node = stack.pop()
        if getattr(node, "is_indirect", False):
            if node.objgen in seen:
                continue
            seen.add(node.objgen)
        if isinstance(node, pikepdf.Dictionary):
            if str(node.get("/Type", "")) in NO_FOLLOW_TYPES:
                continue
            stack.extend(node[k] for k in list(node.keys()))
        elif isinstance(node, pikepdf.Array):
            stack.extend(list(node))
        else:
            text = pdf_string(node)
            if text is not None:
                out.append(text)
    return out


def direct_dicts(obj: Any) -> list[Any]:
    out: list[Any] = []
    stack = [obj.stream_dict if isinstance(obj, pikepdf.Stream) else obj]
    budget = 100_000
    while stack and budget > 0:
        budget -= 1
        node = stack.pop()
        if isinstance(node, pikepdf.Dictionary):
            out.append(node)
            children = [node[k] for k in list(node.keys())]
        elif isinstance(node, pikepdf.Array):
            children = list(node)
        else:
            continue
        stack.extend(c for c in children if not getattr(c, "is_indirect", False))
    return out


def marked_content_strings(owner: Any) -> list[str] | None:
    """Strings in inline property lists of one content stream; ``None`` when
    it can't be read (a verifier that can't look must say so)."""
    try:
        data = owner.read_bytes() if isinstance(owner, pikepdf.Stream) else None
        if data is not None and b"BDC" not in data and b"DP" not in data:
            return []
        out: list[str] = []
        for operands, operator in pikepdf.parse_content_stream(owner):
            if str(operator) in ("BDC", "DP") and operands:
                for node in direct_dicts(operands[-1]):
                    out += [t for k in node.keys() if (t := pdf_string(node[k])) is not None]
        return out
    except pikepdf.PdfError:
        return None


def v10_outside_page(path: Path, needles: list[str]) -> list[str]:
    """Places outside the page text that still hold redacted text: kinds only."""
    if not needles:
        return []
    places: set[str] = set()
    with pikepdf.open(path) as pdf:
        root = pdf.Root

        def any_hit(texts: list[str]) -> bool:
            return any(contains_any(t, needles) for t in texts)

        if "/Outlines" in root and any_hit(strings_below(root.Outlines)):
            places.add("outline")
        if "/PageLabels" in root and any_hit(strings_below(root.PageLabels)):
            places.add("pageLabel")
        if "/Threads" in root and any_hit(strings_below(root.Threads)):
            places.add("thread")
        if "/StructTreeRoot" in root and any_hit(strings_below(root.StructTreeRoot)):
            places.add("structure")
        dests = root.get("/Dests")
        if isinstance(dests, pikepdf.Dictionary) and any_hit([k.lstrip("/") for k in dests.keys()]):
            places.add("destination")
        names = root.get("/Names")
        tree = names.get("/Dests") if isinstance(names, pikepdf.Dictionary) else None
        if tree is not None:
            try:
                if any_hit(list(pikepdf.NameTree(tree).keys())):
                    places.add("destination")
            except Exception:  # noqa: BLE001 - a tree that can't be read can't be cleared
                places.add("destination")
        for obj in pdf.objects:
            if not isinstance(obj, (pikepdf.Dictionary, pikepdf.Stream)):
                continue
            for node in direct_dicts(obj):
                found = [t for k in TEXT_KEYS if (t := pdf_string(node.get(k))) is not None]
                if any_hit(found):
                    places.add("actualText")
            if isinstance(obj, pikepdf.Stream) and obj.stream_dict.get("/Subtype") == "/Form":
                texts = marked_content_strings(obj)
                if texts is None:
                    places.add("unreadableContent")
                elif any_hit(texts):
                    places.add("actualText")
        for page in pdf.pages:
            texts = marked_content_strings(page)
            if texts is None:
                places.add("unreadableContent")
            elif any_hit(texts):
                places.add("actualText")
    return sorted(places)


def v6_fill(doc: Any, regions: dict[int, list[dict[str, Any]]], fill: list[int]) -> list[int]:
    bad = []
    scale = pymupdf.Matrix(V6_DPI / 72, V6_DPI / 72)
    for number, rs in regions.items():
        pix = doc[number].get_pixmap(matrix=scale, alpha=False, annots=False)
        if pix.n != 3:
            pix = pymupdf.Pixmap(pymupdf.csRGB, pix)
        w, h, stride, samples = pix.width, pix.height, pix.stride, pix.samples
        ok = True
        for r in rs:
            x0 = int(r["x0"] * w) + V6_INSET_PX
            x1 = int(r["x1"] * w) - V6_INSET_PX
            y0 = int(r["y0"] * h) + V6_INSET_PX
            y1 = int(r["y1"] * h) - V6_INSET_PX
            if x1 <= x0 or y1 <= y0:
                continue  # too small to sample at this resolution
            exact = bytes(fill) * (x1 - x0)
            for y in range(y0, y1):
                row = samples[y * stride + x0 * 3 : y * stride + x1 * 3]
                if row == exact:
                    continue
                for i in range(0, len(row), 3):
                    if any(abs(row[i + c] - fill[c]) > V6_TOLERANCE for c in range(3)):
                        ok = False
                        break
                if not ok:
                    break
            if not ok:
                break
        if not ok:
            bad.append(number + 1)
    return bad


def v7_single_revision(path: Path) -> bool:
    data = path.read_bytes()
    if data.count(b"startxref") != 1 or data.count(b"%%EOF") != 1:
        return False
    with pikepdf.open(path) as pdf:
        return "/Prev" not in pdf.trailer


def v9_elsewhere(texts: list[str], captured: list[str]) -> tuple[int, list[int]]:
    count, pages = 0, []
    for n, text in enumerate(texts):
        spaced = normalize(text)
        found = sum(spaced.count(normalize(c)) for c in captured if normalize(c))
        if found:
            count += found
            pages.append(n + 1)
    return count, pages


# ------------------------------------------------------------------ main


def verify(job_dir: Path, request: dict[str, Any]) -> dict[str, Any]:
    root = job_dir.resolve()
    path = (root / request["output"]).resolve()
    if root not in path.parents:
        raise ValueError("output outside the job directory")
    terms = [t for t in request.get("terms", []) if normalize(t)]
    captured = list(request.get("captured", []))
    regions = by_page(request.get("regions", []))
    fill = [int(c) for c in request.get("fill", [0, 0, 0])]

    checks: list[dict[str, Any]] = []

    def check(check_id: str, passed: bool, **extra: Any) -> None:
        checks.append({"id": check_id, "passed": bool(passed), **extra})

    doc = pymupdf.open(path)
    pdf = pdfium.PdfDocument(path)
    try:
        bad = v1_mupdf_regions(doc, regions)
        check("V1", not bad, pages=bad)
        bad = v2_pdfium_regions(pdf, regions)
        check("V2", not bad, pages=bad)
        texts_a, texts_b = page_texts_mupdf(doc), page_texts_pdfium(pdf)
        bad = v3_terms(texts_a, texts_b, terms)
        check("V3", not bad, pages=bad)
        check("V4", not v4_raw(path, terms))
        reasons = v5_structure(path)
        check("V5", not reasons, reasons=reasons)
        bad = v6_fill(doc, regions, fill)
        check("V6", not bad, pages=bad)
        check("V7", v7_single_revision(path))
        check("V8", len(doc) == int(request["expectedPages"]) == len(pdf))
        count, pages = v9_elsewhere(texts_a, captured)
        places = v10_outside_page(path, [*terms, *captured])
        check("V10", not places, places=places)
    finally:
        doc.close()
        pdf.close()
    return {"checks": checks, "elsewhere": {"count": count, "pages": pages}}


def main() -> int:
    request = json.loads(sys.stdin.buffer.read().decode("utf-8"))
    report = verify(Path.cwd(), request)
    sys.stdout.write(json.dumps(report))
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
