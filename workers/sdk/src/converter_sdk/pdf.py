"""Shared PDF behavior for pikepdf adapters (spec sections 8.3 and 13.2).

Import this only from PDF adapters: it imports pikepdf at module level, and
the core SDK must stay importable without any engine installed.

``PdfAdapter`` gives every PDF tool the same ``inspect``, ``validate_input``,
``estimate_memory`` and ``render``. Subclasses implement
``validate_params`` and ``execute_conversion``, and use ``save_pdf`` for
every output so the shared saving rules hold everywhere.
"""

from __future__ import annotations

import re
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, ClassVar

try:
    import pikepdf
    from pikepdf import Name
except ImportError as e:  # pragma: no cover - packaging fault, not a user error
    raise ImportError("converter_sdk.pdf needs pikepdf") from e

from .adapter import (
    Adapter,
    InspectReport,
    RenderedPage,
    ResourceEstimate,
    ValidationReport,
)
from .context import JobContext
from .errors import AdapterError
from .protocol import ValidationIssue, WarningItem

MAX_PAGES = 5000
"""Default page limit (blueprint section 6.2). Raising it is a core setting, not a param."""

MAX_FIELD_WALK = 10_000
"""Bound on form fields visited while looking for signatures."""

MB = 1024 * 1024

# --------------------------------------------------------------- inspection


def is_tagged(pdf: pikepdf.Pdf) -> bool:
    return Name.StructTreeRoot in pdf.Root


def has_xfa(pdf: pikepdf.Pdf) -> bool:
    form = pdf.Root.get(Name.AcroForm)
    return isinstance(form, pikepdf.Dictionary) and Name.XFA in form


def _fields(pdf: pikepdf.Pdf) -> Iterator[pikepdf.Dictionary]:
    """Every field dictionary, depth first, bounded and cycle-safe."""
    form = pdf.Root.get(Name.AcroForm)
    if not isinstance(form, pikepdf.Dictionary):
        return
    roots = form.get(Name.Fields)
    if not isinstance(roots, pikepdf.Array):
        return
    stack = list(roots)
    seen: set[tuple[int, int]] = set()
    while stack and len(seen) < MAX_FIELD_WALK:
        field = stack.pop()
        if not isinstance(field, pikepdf.Dictionary):
            continue
        if field.is_indirect:
            if field.objgen in seen:
                continue
            seen.add(field.objgen)
        yield field
        kids = field.get(Name.Kids)
        if isinstance(kids, pikepdf.Array):
            stack.extend(kids)


def is_signed(pdf: pikepdf.Pdf) -> bool:
    """A signature field that carries a value means the file is signed."""
    return any(f.get(Name.FT) == Name.Sig and Name.V in f for f in _fields(pdf))


def field_names(pdf: pikepdf.Pdf) -> set[str]:
    """Fully qualified names of terminal fields (``parent.child``)."""
    names: set[str] = set()
    form = pdf.Root.get(Name.AcroForm)
    if not isinstance(form, pikepdf.Dictionary):
        return names
    roots = form.get(Name.Fields)
    if not isinstance(roots, pikepdf.Array):
        return names
    stack: list[tuple[Any, str]] = [(f, "") for f in roots]
    seen: set[tuple[int, int]] = set()
    while stack and len(seen) < MAX_FIELD_WALK:
        field, prefix = stack.pop()
        if not isinstance(field, pikepdf.Dictionary):
            continue
        if field.is_indirect:
            if field.objgen in seen:
                continue
            seen.add(field.objgen)
        part = str(field.get(Name.T, ""))
        full = f"{prefix}.{part}" if prefix and part else (part or prefix)
        kids = field.get(Name.Kids)
        named_kids = isinstance(kids, pikepdf.Array) and any(
            isinstance(k, pikepdf.Dictionary) and Name.T in k for k in kids
        )
        if named_kids:
            stack.extend((k, full) for k in kids)
        elif full:
            names.add(full)
    return names


def has_active_content(pdf: pikepdf.Pdf) -> bool:
    """JavaScript, an automatic action, or attachments (warning ACTIVE_CONTENT_FOUND)."""
    root = pdf.Root
    names = root.get(Name.Names)
    if isinstance(names, pikepdf.Dictionary) and (
        Name.JavaScript in names or Name.EmbeddedFiles in names
    ):
        return True
    if Name.OpenAction in root or Name.AA in root:
        return True
    return False


def encryption_state(ctx: JobContext, path: Path) -> str:
    """``none``, ``permissionsOnly`` or ``openPassword``, without needing the password."""
    try:
        with pikepdf.open(path) as pdf:
            return "permissionsOnly" if pdf.is_encrypted else "none"
    except pikepdf.PasswordError:
        return "openPassword"
    except pikepdf.PdfError:
        raise AdapterError("E_INPUT_CORRUPT") from None


# -------------------------------------------------------------- page ranges

_RANGE = re.compile(r"^\s*(\d*)\s*(-?)\s*(\d*)\s*$")


class RangeError(ValueError):
    """``reason`` is safe to show: it names the problem, not the input."""

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


def parse_page_ranges(spec: str, total: int) -> list[tuple[int, int]]:
    """Parse the ``pageRanges`` widget's syntax, e.g. ``1-3, 5, 8-``.

    Returns 1-based inclusive ``(first, last)`` pairs in the order given.
    ``8-`` runs to the last page and ``-3`` starts at the first.
    """
    if len(spec) > 10_000:
        raise RangeError("tooLong")
    parts = [p for p in spec.split(",") if p.strip()]
    if not parts:
        raise RangeError("empty")
    ranges: list[tuple[int, int]] = []
    for part in parts:
        m = _RANGE.match(part)
        if not m or (not m.group(1) and not m.group(3)):
            raise RangeError("syntax")
        first_s, dash, last_s = m.groups()
        first = int(first_s) if first_s else 1
        if dash:
            last = int(last_s) if last_s else total
        else:
            last = first
        if first < 1 or last > total:
            raise RangeError("outOfBounds")
        if first > last:
            raise RangeError("reversed")
        ranges.append((first, last))
    return ranges


def pages_in(ranges: list[tuple[int, int]]) -> list[int]:
    """Distinct 1-based page numbers covered by ``ranges``, ascending."""
    return sorted({p for first, last in ranges for p in range(first, last + 1)})


def param_issue(param: str, reason: str, **detail: Any) -> ValidationIssue:
    """An invalid parameter. The catalog has no parameter error code yet, so
    this uses ``E_INTERNAL`` with ``param`` set; the core and UI key on ``param``."""
    return ValidationIssue(code="E_INTERNAL", param=param, detail={"reason": reason, **detail})


# ---------------------------------------------------------------- bookmarks


def _page_index(src: pikepdf.Pdf) -> dict[tuple[int, int], int]:
    return {p.obj.objgen: i for i, p in enumerate(src.pages)}


def _named_dest(src: pikepdf.Pdf, name: Any) -> Any:
    key = str(name)
    dests = src.Root.get(Name.Dests)
    if isinstance(dests, pikepdf.Dictionary) and ("/" + key) in dests:
        return dests["/" + key]
    names = src.Root.get(Name.Names)
    if isinstance(names, pikepdf.Dictionary) and Name.Dests in names:
        tree = pikepdf.NameTree(names.Dests)
        if key in tree:
            return tree[key]
    return None


def dest_page(src: pikepdf.Pdf, item: Any, index: dict[tuple[int, int], int]) -> int | None:
    """0-based source page an outline item points at, or None if it can't be resolved."""
    dest = item.destination
    if dest is None and item.action is not None:
        action = item.action
        if action.get(Name.S) == Name.GoTo:
            dest = action.get(Name.D)
    for _ in range(4):  # named destinations can point at dicts with /D
        if isinstance(dest, (pikepdf.Name, pikepdf.String)) or isinstance(dest, str):
            dest = _named_dest(src, dest)
        elif isinstance(dest, pikepdf.Dictionary):
            dest = dest.get(Name.D)
        else:
            break
    if isinstance(dest, pikepdf.Array) and len(dest) > 0:
        target = dest[0]
        if isinstance(target, pikepdf.Dictionary) and target.is_indirect:
            return index.get(target.objgen)
        if isinstance(target, int):
            return int(target) if 0 <= int(target) < len(src.pages) else None
    return None


def copy_outline(
    src: pikepdf.Pdf,
    page_map: dict[int, int],
    into: list[Any],
) -> int:
    """Copy ``src``'s bookmarks into the list ``into``, pointing at new page indices.

    ``page_map`` maps 0-based source pages to 0-based destination pages.
    Items whose page isn't in the map are dropped; their children are kept
    and move up a level. Returns how many items were dropped.
    """
    index = _page_index(src)
    dropped = 0

    def walk(items: list[Any], out: list[Any], depth: int) -> None:
        nonlocal dropped
        for item in items:
            page = dest_page(src, item, index)
            target = page_map.get(page) if page is not None else None
            if target is None:
                dropped += 1
                if depth < 32:
                    walk(list(item.children), out, depth + 1)
                continue
            new = pikepdf.OutlineItem(str(item.title), target)
            out.append(new)
            if depth < 32:
                walk(list(item.children), new.children, depth + 1)

    with src.open_outline(strict=False) as outline:
        walk(list(outline.root), into, 0)
    return dropped


# --------------------------------------------------------------- placement


def displayed_box(page: pikepdf.Page) -> tuple[float, float, float, float, int]:
    """(x0, y0, width, height, rotate) of the visible area in user space."""
    box = page.cropbox
    x0, x1 = sorted((float(box[0]), float(box[2])))
    y0, y1 = sorted((float(box[1]), float(box[3])))
    rotate = int(page.obj.get(pikepdf.Name.Rotate, 0)) % 360
    rotate = rotate - rotate % 90
    return x0, y0, x1 - x0, y1 - y0, rotate


def placement(x0: float, y0: float, w: float, h: float, rotate: int) -> list[float]:
    """The ``cm`` matrix mapping displayed coordinates (origin at the visible
    bottom-left) into page user space, for a clockwise /Rotate."""
    if rotate == 90:
        return [0, 1, -1, 0, x0 + w, y0]
    if rotate == 180:
        return [-1, 0, 0, -1, x0 + w, y0 + h]
    if rotate == 270:
        return [0, -1, 1, 0, x0, y0 + h]
    return [1, 0, 0, 1, x0, y0]


# ------------------------------------------------------------------- saving


def save_pdf(
    ctx: JobContext,
    pdf: pikepdf.Pdf,
    path: Path,
    *,
    encryption: pikepdf.Encryption | None = None,
    check_password: str = "",
    recompress_flate: bool = False,
) -> int:
    """Save with the shared rules in spec 13.2, then check the result.

    Full rewrite (never incremental), compressed streams, object streams,
    and a deterministic /ID when unencrypted so repeat runs give identical
    bytes. The output is reopened and checked like ``qpdf --check``; any
    problem fails the job with ``E_OUTPUT_INVALID``. Returns the page count.
    """
    ctx.check_cancelled()
    repaired = bool(pdf.get_warnings())  # cleared by save; read it first

    def on_progress(percent: int) -> None:
        # Raising here aborts qpdf's writer, so even one large save honors cancel.
        ctx.check_cancelled()
        ctx.progress("save", percent, 100)

    pdf.save(
        path,
        progress=on_progress,
        compress_streams=True,
        object_stream_mode=pikepdf.ObjectStreamMode.generate,
        deterministic_id=encryption is None,
        encryption=encryption if encryption is not None else False,
        fix_metadata_version=False,
        recompress_flate=recompress_flate,
    )
    problems: list[str] | None
    try:
        with pikepdf.open(path, password=check_password) as written:
            problems = written.check_pdf_syntax()
            pages = len(written.pages)
    except pikepdf.PdfError:
        problems, pages = None, 0
    # Only problem counts leave the worker: messages can quote document content
    # and paths.
    if problems is None or problems:
        if repaired:
            # qpdf had to repair this document on open and the damage survived
            # the rewrite: the input is at fault, not the adapter.
            raise AdapterError("E_INPUT_CORRUPT", {"reason": "damaged"})
        detail = {"reason": "unreadable"} if problems is None else {"problems": len(problems)}
        raise AdapterError("E_OUTPUT_INVALID", detail)
    return pages


def copy_into(dst: pikepdf.Pdf, src: pikepdf.Pdf, obj: pikepdf.Object) -> pikepdf.Object:
    """``copy_foreign`` that also accepts direct objects (it makes them indirect in
    ``src`` first; ``src`` is an input and is never saved)."""
    return dst.copy_foreign(obj if obj.is_indirect else src.make_indirect(obj))


def flatten_forms(pdf: pikepdf.Pdf) -> None:
    """Bake form field appearances into page content (shared rule 13.2)."""
    pdf.generate_appearance_streams()
    pdf.flatten_annotations("all")
    if Name.AcroForm in pdf.Root:
        del pdf.Root[Name.AcroForm]


# ----------------------------------------------------------- hidden layers


def hidden_ocgs(pdf: pikepdf.Pdf) -> set[tuple[int, int]]:
    """Optional-content groups that are off in the default configuration."""
    props = pdf.Root.get(Name.OCProperties)
    if not isinstance(props, pikepdf.Dictionary):
        return set()
    every = {o.objgen for o in props.get(Name.OCGs, pikepdf.Array()) if o.is_indirect}
    default = props.get(Name.D)
    if not isinstance(default, pikepdf.Dictionary):
        return set()
    off = {o.objgen for o in default.get(Name.OFF, pikepdf.Array()) if o.is_indirect}
    if default.get(Name.BaseState) == Name.OFF:
        on = {o.objgen for o in default.get(Name.ON, pikepdf.Array()) if o.is_indirect}
        off |= every - on
    return off


def _is_hidden(oc: Any, hidden: set[tuple[int, int]]) -> bool:
    """An OCG or OCMD that involves any hidden group. Erring towards hidden
    removes more, never less, which is the safe side for redaction."""
    if not isinstance(oc, pikepdf.Dictionary):
        return False
    if oc.is_indirect and oc.objgen in hidden:
        return True
    groups = oc.get(Name.OCGs)
    if isinstance(groups, pikepdf.Dictionary):
        groups = [groups]
    if isinstance(groups, (pikepdf.Array, list)):
        return any(g.is_indirect and g.objgen in hidden for g in groups)
    return False


def _strip_hidden_content(
    pdf: pikepdf.Pdf, owner: Any, resources: Any, hidden: set[tuple[int, int]], seen: set
) -> None:
    """Remove ``/OC … BDC … EMC`` sections and ``Do`` of XObjects that belong
    to hidden groups, in ``owner``'s content and in nested form XObjects."""
    props = resources.get(Name.Properties) if isinstance(resources, pikepdf.Dictionary) else None
    xobjects = resources.get(Name.XObject) if isinstance(resources, pikepdf.Dictionary) else None
    out = []
    skip_depth = 0
    depth = 0
    changed = False
    for operands, operator in pikepdf.parse_content_stream(owner):
        op = str(operator)
        if op in ("BDC", "BMC"):
            depth += 1
            if skip_depth:
                continue
            if op == "BDC" and len(operands) == 2 and operands[0] == Name.OC:
                tag = operands[1]
                oc = props.get(tag) if isinstance(tag, pikepdf.Name) and props is not None else tag
                if _is_hidden(oc, hidden):
                    skip_depth = depth
                    changed = True
                    continue
        elif op == "EMC":
            if skip_depth:
                if depth == skip_depth:
                    skip_depth = 0
                depth -= 1
                continue
            depth -= 1
        elif skip_depth:
            continue
        elif op == "Do" and xobjects is not None:
            xobj = xobjects.get(operands[0])
            if isinstance(xobj, pikepdf.Stream):
                if _is_hidden(xobj.stream_dict.get(Name.OC), hidden):
                    changed = True
                    continue
                if xobj.stream_dict.get(Name.Subtype) == Name.Form and xobj.objgen not in seen:
                    seen.add(xobj.objgen)
                    _strip_hidden_content(
                        pdf, xobj, xobj.stream_dict.get(Name.Resources), hidden, seen
                    )
        out.append((operands, operator))
    if changed:
        data = pikepdf.unparse_content_stream(out)
        if isinstance(owner, pikepdf.Page):
            owner.obj.Contents = pdf.make_stream(data)
        else:
            owner.write(data)


def remove_hidden_layers(pdf: pikepdf.Pdf) -> bool:
    """Spec 13.8 strip step: content in layers that are off by default goes.
    Returns True if the document had optional content at all."""
    if Name.OCProperties not in pdf.Root:
        return False
    hidden = hidden_ocgs(pdf)
    if hidden:
        seen: set[tuple[int, int]] = set()
        for page in pdf.pages:
            _strip_hidden_content(pdf, page, page.obj.get(Name.Resources), hidden, seen)
    # What's left is visible content; drop the layer machinery so no viewer
    # can toggle anything.
    del pdf.Root[Name.OCProperties]
    return True


# ---------------------------------------------------------- active content

_ACTION_KINDS = {
    "/JavaScript": "javascript",
    "/Launch": "launch",
    "/SubmitForm": "forms",
    "/ImportData": "forms",
    "/GoToR": "remote",
    "/GoToE": "remote",
    "/Rendition": "media",
    "/RichMediaExecute": "media",
    "/Sound": "media",
    "/Movie": "media",
    "/Hide": "hide",
}
"""Action types that never survive "make safe to open". ``SetOCGState`` stays."""

_ANNOT_KINDS = {
    "/FileAttachment": "embedded",
    "/RichMedia": "media",
    "/Screen": "media",
    "/Movie": "media",
    "/Sound": "media",
    "/3D": "media",
}

MAX_ACTION_CHAIN = 64
MAX_DIRECT_DEPTH = 64


class _Sweep:
    """One pass over every object, either counting or removing active content.

    Counting and removing share this code so a detector and a remover can
    never disagree about what "active" means.
    """

    def __init__(
        self,
        pdf: pikepdf.Pdf,
        remove: bool,
        remove_links: bool,
        check: Callable[[], None] | None = None,
    ) -> None:
        self.pdf = pdf
        self.check = check
        self.remove = remove
        self.remove_links = remove_links
        self.counts: dict[str, int] = {}

    def hit(self, kind: str, n: int = 1, form: bool = False) -> None:
        if n <= 0:
            return
        self.counts[kind] = self.counts.get(kind, 0) + n
        if form:
            self.counts["formActions"] = self.counts.get("formActions", 0) + n

    def kind_of(self, action: Any) -> str | None:
        if not isinstance(action, pikepdf.Dictionary):
            return None
        s = str(action.get(Name.S, ""))
        if s == "/URI" and self.remove_links:
            return "links"
        return _ACTION_KINDS.get(s)

    def next_chain(self, action: pikepdf.Dictionary, form: bool, depth: int = 0) -> None:
        """Actions chained with /Next: drop the bad ones, keep the rest."""
        nxt = action.get(Name.Next)
        if nxt is None or depth > MAX_ACTION_CHAIN:
            return
        if isinstance(nxt, pikepdf.Dictionary):
            kind = self.kind_of(nxt)
            if kind:
                self.hit(kind, form=form)
                if self.remove:
                    del action[Name.Next]
            else:
                self.next_chain(nxt, form, depth + 1)
        elif isinstance(nxt, pikepdf.Array):
            kept = []
            changed = False
            for item in list(nxt)[:1000]:
                kind = self.kind_of(item)
                if kind:
                    self.hit(kind, form=form)
                    changed = True
                    continue
                if isinstance(item, pikepdf.Dictionary):
                    self.next_chain(item, form, depth + 1)
                kept.append(item)
            if changed and self.remove:
                action[Name.Next] = pikepdf.Array(kept)

    def action_value(self, holder: Any, key: Name, form: bool) -> None:
        action = holder.get(key)
        kind = self.kind_of(action)
        if kind:
            self.hit(kind, form=form)
            if self.remove:
                del holder[key]
        elif isinstance(action, pikepdf.Dictionary):
            self.next_chain(action, form)

    def holder(self, d: Any) -> None:
        subtype = str(d.get(Name.Subtype, ""))
        form = subtype == "/Widget" or Name.FT in d
        if Name.AA in d:
            aa = d[Name.AA]
            self.hit("aa", form=form)
            if isinstance(aa, pikepdf.Dictionary):
                for key in list(aa.keys()):
                    kind = self.kind_of(aa.get(key))
                    if kind:
                        self.hit(kind, form=form)
            if self.remove:
                del d[Name.AA]
        if Name.A in d:
            self.action_value(d, Name.A, form)
        if Name.AF in d:
            af = d[Name.AF]
            self.hit("embedded", len(af) if isinstance(af, pikepdf.Array) else 1)
            if self.remove:
                del d[Name.AF]
        annots = d.get(Name.Annots)
        if isinstance(annots, pikepdf.Array):
            for i in range(len(annots) - 1, -1, -1):
                annot = annots[i]
                if not isinstance(annot, pikepdf.Dictionary):
                    continue
                kind = _ANNOT_KINDS.get(str(annot.get(Name.Subtype, "")))
                if kind:
                    self.hit(kind)
                    if self.remove:
                        del annots[i]

    def root(self) -> None:
        root = self.pdf.Root
        action = root.get(Name.OpenAction)
        if isinstance(action, pikepdf.Dictionary):
            kind = self.kind_of(action)
            plain_goto = action.get(Name.S) == Name.GoTo
            if kind:
                self.hit(kind)
            elif plain_goto:
                self.next_chain(action, False)
            if kind or not plain_goto:
                self.hit("aa")
                if self.remove:
                    del root[Name.OpenAction]
        names = root.get(Name.Names)
        if isinstance(names, pikepdf.Dictionary):
            if Name.JavaScript in names:
                self.hit("javascript")
                if self.remove:
                    del names[Name.JavaScript]
            if Name.EmbeddedFiles in names:
                self.hit("embedded", _tree_size(names[Name.EmbeddedFiles]))
                if self.remove:
                    del names[Name.EmbeddedFiles]
        form = root.get(Name.AcroForm)
        if isinstance(form, pikepdf.Dictionary) and Name.XFA in form:
            self.hit("xfa", form=True)
            if self.remove:
                del form[Name.XFA]

    def run(self) -> dict[str, int]:
        self.root()
        for n, obj in enumerate(self.pdf.objects):
            if self.check is not None and n % 256 == 0:
                self.check()
            if isinstance(obj, pikepdf.Stream):
                stack: list[Any] = [obj.stream_dict]
            elif isinstance(obj, (pikepdf.Dictionary, pikepdf.Array)):
                stack = [obj]
            else:
                continue
            depth_budget = MAX_DIRECT_DEPTH * 1000
            while stack and depth_budget > 0:
                depth_budget -= 1
                node = stack.pop()
                if isinstance(node, pikepdf.Dictionary):
                    self.holder(node)
                    children = [node[k] for k in list(node.keys())]
                else:
                    children = list(node)
                stack.extend(
                    c
                    for c in children
                    if isinstance(c, (pikepdf.Dictionary, pikepdf.Array)) and not c.is_indirect
                )
        return self.counts


def _tree_size(tree: Any) -> int:
    try:
        return max(1, len(pikepdf.NameTree(tree)))
    except Exception:  # noqa: BLE001 - a damaged tree still counts as present
        return 1


def scan_active_content(pdf: pikepdf.Pdf) -> dict[str, int]:
    """Read-only counts of what ``remove_active_content`` would remove, by kind:
    ``javascript``, ``aa`` (automatic actions), ``launch``, ``forms``, ``remote``,
    ``media``, ``hide``, ``embedded``, ``xfa``. Plain web links are not counted."""
    return _Sweep(pdf, remove=False, remove_links=False).run()


def remove_active_content(
    pdf: pikepdf.Pdf,
    *,
    remove_links: bool = False,
    check: Callable[[], None] | None = None,
) -> dict[str, int]:
    """Remove scripts, automatic and external actions, attachments, embedded
    media and XFA. Returns counts by kind (see ``scan_active_content``), plus
    ``links`` when ``remove_links`` removes ``/URI`` actions, and ``formActions``
    for how many of the removals were on form fields. ``check`` is called now and
    then (pass ``ctx.check_cancelled``) so a long document stays cancellable."""
    return _Sweep(pdf, remove=True, remove_links=remove_links, check=check).run()


# ------------------------------------------------------------------ adapter


@dataclass
class OpenedInput:
    path: Path
    relative: str
    pdf: pikepdf.Pdf
    pages: int


class PdfAdapter(Adapter):
    """Base for pikepdf adapters. Subclasses set ``adapter_id`` and ``fidelity``
    and implement ``validate_params`` and ``execute_conversion``."""

    base_ram_bytes: ClassVar[int] = 96 * MB
    """Interpreter plus pikepdf, measured on macOS and Linux. The blueprint's
    64 MB left out Python itself."""
    ram_per_input_byte: ClassVar[float] = 0.3
    disk_factor: ClassVar[float] = 2.0
    max_pages: ClassVar[int] = MAX_PAGES

    # --- hooks for subclasses ------------------------------------------

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        """Adapter-specific parameter checks against the opened inputs."""
        return []

    def check_input(self, ctx: JobContext, opened: OpenedInput) -> ValidationIssue | None:
        """Per-input precondition, e.g. pdf-protect refuses encrypted input."""
        return None

    # --- shared contract -----------------------------------------------

    def relative(self, ctx: JobContext, path: Path) -> str:
        return path.relative_to(ctx.job_dir.resolve()).as_posix()

    def inspect(self, ctx: JobContext, input: Path) -> InspectReport:
        encryption = encryption_state(ctx, input)
        if encryption == "openPassword" and (ctx.secrets is None or ctx.secrets.password is None):
            # Facts we can give without the password.
            return InspectReport(pages=0, encryption="openPassword")
        with ctx.open_pdf(input) as pdf:
            warnings = []
            if has_xfa(pdf):
                warnings.append(WarningItem(code="XFA_UNSUPPORTED"))
            if has_active_content(pdf):
                warnings.append(WarningItem(code="ACTIVE_CONTENT_FOUND"))
            from . import reveal  # noqa: PLC0415

            return InspectReport(
                pages=len(pdf.pages),
                encryption=encryption,  # type: ignore[arg-type]
                signed=is_signed(pdf),
                warnings=warnings,
                findings=reveal.open_pdf_findings(pdf),
            )

    def validate_input(self, ctx: JobContext, params: dict[str, Any]) -> ValidationReport:
        report = ValidationReport()
        opened: list[OpenedInput] = []
        total_bytes = 0
        warned: set[str] = set()

        def warn_once(code: str) -> None:
            if code not in warned:
                warned.add(code)
                report.warnings.append(WarningItem(code=code))

        try:
            for path in ctx.inputs:
                relative = self.relative(ctx, path)
                if not path.is_file():
                    report.errors.append(ValidationIssue(code="E_INPUT_CORRUPT", input=relative))
                    continue
                total_bytes += path.stat().st_size
                try:
                    pdf = ctx.open_pdf(path)
                except AdapterError as e:
                    report.errors.append(ValidationIssue(code=e.code, input=relative))
                    continue
                item = OpenedInput(path, relative, pdf, len(pdf.pages))
                opened.append(item)
                if item.pages == 0:
                    report.errors.append(ValidationIssue(code="E_INPUT_CORRUPT", input=relative))
                elif item.pages > self.max_pages:
                    report.errors.append(
                        ValidationIssue(
                            code="E_LIMIT_EXCEEDED",
                            input=relative,
                            detail={"limit": "pages", "max": self.max_pages},
                        )
                    )
                if (issue := self.check_input(ctx, item)) is not None:
                    report.errors.append(issue)
                if is_signed(pdf):
                    warn_once("SIGNATURE_INVALIDATED")
                if has_xfa(pdf):
                    warn_once("XFA_UNSUPPORTED")
                if has_active_content(pdf):
                    warn_once("ACTIVE_CONTENT_FOUND")
            if not report.errors:
                report.errors.extend(self.validate_params(ctx, params, opened))
        finally:
            for item in opened:
                item.pdf.close()
        report.metadata = {
            "inputBytes": total_bytes,
            "pages": [o.pages for o in opened],
        }
        return report

    def estimate_memory(
        self, ctx: JobContext, params: dict[str, Any], metadata: dict[str, Any]
    ) -> ResourceEstimate:
        input_bytes = int(metadata.get("inputBytes", 0))
        pages = sum(int(p) for p in metadata.get("pages", []))
        return ResourceEstimate(
            ram_bytes=self.base_ram_bytes + int(self.ram_per_input_byte * input_bytes),
            disk_bytes=int(self.disk_factor * input_bytes),
            cost_units=max(1, pages),
        )

    def render(self, ctx: JobContext, input: Path, page: int, dpi: int, out: Path) -> RenderedPage:
        """Preview rendering through pdfium (spec C2: previews are worker-rendered PNGs)."""
        try:
            import pypdfium2 as pdfium  # noqa: PLC0415 - only preview workers need it
        except ImportError:
            raise AdapterError("E_INSTALL_CORRUPT", {"missing": "pypdfium2"}) from None
        password = None
        if ctx.secrets is not None and ctx.secrets.password is not None:
            password = ctx.secrets.password.get_secret_value()
        try:
            doc = pdfium.PdfDocument(input, password=password)
        except pdfium.PdfiumError:
            code = "E_PASSWORD_WRONG" if password else "E_INPUT_CORRUPT"
            if encryption_state(ctx, input) == "openPassword" and not password:
                code = "E_PASSWORD_REQUIRED"
            raise AdapterError(code) from None
        finally:
            del password
        try:
            if not 1 <= page <= len(doc):
                raise AdapterError("E_INTERNAL", {"reason": "pageOutOfRange"}, retryable=False)
            image = doc[page - 1].render(scale=dpi / 72).to_pil()
            out.parent.mkdir(parents=True, exist_ok=True)
            image.save(out, format="PNG")
            return RenderedPage(width_px=image.width, height_px=image.height)
        finally:
            doc.close()

    # --- helpers for execute -------------------------------------------

    def open_inputs(self, ctx: JobContext) -> list[pikepdf.Pdf]:
        """Open every input in order. Callers close them (or use ``closing``)."""
        return [ctx.open_pdf(path) for path in ctx.inputs]

    def warn_shared(self, ctx: JobContext, pdfs: list[pikepdf.Pdf]) -> None:
        """Execution-time warnings from shared rule 13.2."""
        if any(is_signed(p) for p in pdfs):
            ctx.warn("SIGNATURE_INVALIDATED")
        if any(has_xfa(p) for p in pdfs):
            ctx.warn("XFA_UNSUPPORTED")
