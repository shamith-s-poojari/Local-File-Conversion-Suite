"""Active content: find it, and take it out (content disarm and reconstruction).

"Active" means anything that can run, call out or hide a payload when a file
is opened: scripts, macros, ActiveX, embedded files and objects, automatic
actions, links that load things from outside the file.

Public functions:

``active_findings(path, fmt)`` and ``pdf_active_findings(pdf)``
    Read-only. A ``Finding`` list: ``activeContent``, ``macros``,
    ``externalLinks``, ``embeddedFiles``. Plain web links are not findings.
``disarm_ooxml(src, dst)`` and ``disarm_markup(src, dst, fmt)``
    Write a copy without them and return counts of what was removed. PDFs are
    handled by ``converter_sdk.pdf.remove_active_content``.

The detectors and the removers walk the same structures with the same rules,
so a file a remover has cleaned never reports findings (and when it does,
something was missed and the adapter's verifier fails the job).

Finding ``detail`` strings and error details only ever hold kinds and counts,
never document text. XML is parsed without entities, DTDs or network access;
a DOCTYPE in an OOXML part or SVG is hostile and refused (``E_INPUT_CORRUPT``).
"""

from __future__ import annotations

import posixpath
import re
import shutil
import zipfile
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .errors import AdapterError
from .markup import _strip_declaration, check_zip, read_text
from .protocol import Finding

OOXML_FORMATS = ("docx", "xlsx", "pptx", "docm", "xlsm", "pptm")
OOXML_MEMBER_MAX = 256 * 1024 * 1024
OOXML_TOTAL_MAX = 1024 * 1024 * 1024
MAX_NODES = 5_000_000
"""Bound on elements visited in one markup file."""

W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
SS_NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
REL_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
PKG_REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
XML_NS = "http://www.w3.org/XML/1998/namespace"
CFB_MAGIC = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"

WEB_SCHEMES = {"http", "https", "mailto", "tel"}
_CONTROL = re.compile(r"[\x00-\x20\x7f]")


def _plural(n: int, one: str, many: str) -> str:
    return f"{n} {one if n == 1 else many}"


def _outside(n: int) -> str:
    return f"Loads {_plural(n, 'thing', 'things')} from outside the file"


# ---------------------------------------------------------------- XML


def _etree() -> Any:
    from lxml import etree  # noqa: PLC0415 - engine imported on demand

    return etree


def parse_xml(data: bytes) -> Any:
    """Parse untrusted XML: no entities, DTD or network. A DOCTYPE is refused."""
    etree = _etree()
    parser = etree.XMLParser(
        resolve_entities=False, no_network=True, load_dtd=False, huge_tree=False
    )
    try:
        root = etree.fromstring(data, parser=parser)
    except etree.XMLSyntaxError:
        raise AdapterError("E_INPUT_CORRUPT") from None
    if root is None or root.getroottree().docinfo.doctype or b"<!ENTITY" in data[:100_000]:
        raise AdapterError("E_INPUT_CORRUPT")
    return root


def _serialize(root: Any) -> bytes:
    return _etree().tostring(root, xml_declaration=True, encoding="UTF-8", standalone=True)


def _local(name: Any) -> str:
    """Lower-case local name of an element or attribute (``{ns}x`` or ``p:x``)."""
    if not isinstance(name, str):
        return ""
    return name.rpartition("}")[2].rpartition(":")[2].lower()


def _drop(el: Any) -> None:
    """Remove ``el`` and keep the text that followed it."""
    parent = el.getparent()
    if parent is None:
        return
    tail = el.tail
    if tail:
        prev = el.getprevious()
        if prev is not None:
            prev.tail = (prev.tail or "") + tail
        else:
            parent.text = (parent.text or "") + tail
    parent.remove(el)


# ------------------------------------------------------------- URLs


def _scheme(url: str) -> str:
    """Lower-case scheme of ``url`` after removing the control characters
    browsers ignore. ``""`` for relative references; ``"//"`` for network paths
    and ``"unc"`` for ``\\\\host`` paths."""
    u = _CONTROL.sub("", url)
    if u.startswith(("//", "\\\\", "\\/", "/\\")):
        return "//"
    try:
        return urlsplit(u).scheme.lower()
    except ValueError:
        return "invalid"


def _is_web(url: str) -> bool:
    return _scheme(url) in WEB_SCHEMES


# ------------------------------------------------------------- PDF


def pdf_active_findings(pdf: Any) -> list[Finding]:
    """Findings for an open ``pikepdf.Pdf`` (the caller closes it)."""
    from .pdf import scan_active_content  # noqa: PLC0415 - imports pikepdf

    c = scan_active_content(pdf)
    found: list[Finding] = []
    if c.get("javascript"):
        found.append(Finding(kind="activeContent", detail="JavaScript"))
    if c.get("aa"):
        found.append(Finding(kind="activeContent", detail="Runs actions when opened"))
    if c.get("launch"):
        found.append(Finding(kind="activeContent", detail="Opens another program"))
    if c.get("forms"):
        found.append(Finding(kind="activeContent", detail="Sends or loads form data"))
    if c.get("hide"):
        found.append(Finding(kind="activeContent", detail="Hides or shows form fields"))
    if c.get("remote"):
        found.append(Finding(kind="externalLinks", detail="Opens other files"))
    if c.get("embedded"):
        found.append(
            Finding(
                kind="embeddedFiles",
                detail=_plural(c["embedded"], "embedded file", "embedded files"),
            )
        )
    if c.get("media"):
        found.append(Finding(kind="activeContent", detail="Embedded media"))
    if c.get("xfa"):
        found.append(Finding(kind="activeContent", detail="XFA form"))
    return found


def _pdf_findings(path: Path) -> list[Finding]:
    try:
        import pikepdf  # noqa: PLC0415
    except ImportError:
        raise AdapterError("E_INSTALL_CORRUPT", {"missing": "pikepdf"}) from None
    try:
        with pikepdf.open(path) as pdf:
            return pdf_active_findings(pdf)
    except pikepdf.PasswordError:
        raise AdapterError("E_PASSWORD_REQUIRED") from None
    except pikepdf.PdfError:
        raise AdapterError("E_INPUT_CORRUPT") from None


# ------------------------------------------------------------ OOXML

_DROP_ELEMENTS = {
    "externaldata",
    "attachedtemplate",
    "subdoc",
    "externalreference",
    "oleobject",
    "oleobj",
    "control",
    "frame",
    "hlinkclick",
    "hlinkhover",
    "hlinkmouseover",
}
"""Elements whose only job is to point at a relationship that was removed."""
_PRUNE_EMPTY = {"oleobjects", "controls", "externalreferences"}

_FIELD = re.compile(
    r"\s*(DDEAUTO|DDE|LINK|INCLUDETEXT|INCLUDEPICTURE|IMPORT|DATABASE|MACROBUTTON)\b", re.I
)
_PROGRAM_ACTION = re.compile(rb"ppaction://(?:program|macro|ole|hlinkfile)", re.I)
_PROGRAM_ACTION_STR = re.compile(r"ppaction://(?:program|macro|ole|hlinkfile)", re.I)
_OPC = "application/vnd.openxmlformats-officedocument."
_MACROENABLED = {
    "application/vnd.ms-word.document.macroEnabled.main+xml": _OPC
    + "wordprocessingml.document.main+xml",
    "application/vnd.ms-word.template.macroEnabledTemplate.main+xml": _OPC
    + "wordprocessingml.template.main+xml",
    "application/vnd.ms-excel.sheet.macroEnabled.main+xml": _OPC + "spreadsheetml.sheet.main+xml",
    "application/vnd.ms-excel.template.macroEnabled.main+xml": _OPC
    + "spreadsheetml.template.main+xml",
    "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml": _OPC
    + "presentationml.presentation.main+xml",
    "application/vnd.ms-powerpoint.slideshow.macroEnabled.main+xml": _OPC
    + "presentationml.slideshow.main+xml",
    "application/vnd.ms-powerpoint.template.macroEnabled.main+xml": _OPC
    + "presentationml.template.main+xml",
}
_VBA_CONTENT_TYPE = "application/vnd.ms-office.vbaProject"


def _key(name: str) -> str:
    return name.replace("\\", "/").lstrip("/").lower()


def _is_vba(key: str) -> bool:
    base = key.rpartition("/")[2]
    return (base.startswith("vbaproject") and base.endswith(".bin")) or base == "vbadata.xml"


class _Package:
    """An OOXML package read through the central directory: part classes and
    relationships, with nothing extracted to disk."""

    def __init__(self, zf: zipfile.ZipFile, remove_links: bool) -> None:
        self.zf = zf
        self.remove_links = remove_links
        self.infos = check_zip(zf, member_max=OOXML_MEMBER_MAX, total_max=OOXML_TOTAL_MAX)
        keys = [_key(i.filename) for i in self.infos]
        if len(set(keys)) != len(keys):
            raise AdapterError("E_INPUT_CORRUPT")  # two parts of one name: readers disagree
        self.by_key = {_key(i.filename): i for i in self.infos}
        self.macros: set[str] = set()
        self.activex: set[str] = set()
        self.embeddings: set[str] = set()
        self.macrosheets: set[str] = set()
        self.workbook_links: set[str] = set()
        for k in self.by_key:
            if k.endswith(".rels") or k.endswith("/"):
                continue
            if _is_vba(k):
                self.macros.add(k)
            elif "/activex/" in "/" + k:
                self.activex.add(k)
            elif "/embeddings/" in "/" + k:
                self.embeddings.add(k)
            elif k.startswith("xl/macrosheets/"):
                self.macrosheets.add(k)
            elif k.startswith("xl/externallinks/"):
                self.workbook_links.add(k)
        # Parts that go, and the relationship files that belonged to them.
        self.removed = self.macros | self.activex | self.embeddings | self.workbook_links
        self.removed |= {self._rels_of(k) for k in self.removed}
        # rels part -> [(rId, type, target, external)]; counts for findings.
        self.rels: dict[str, list[tuple[str, str, str, bool]]] = {}
        self.dropped_rids: dict[str, set[str]] = {}  # source part key -> rIds to neutralize
        self.drop_rel_ids: dict[str, set[str]] = {}  # rels part key -> rIds to delete
        self.external = 0
        self.web_links = 0
        self._read_rels()

    @staticmethod
    def _rels_of(part: str) -> str:
        directory, _, base = part.rpartition("/")
        return f"{directory}/_rels/{base}.rels" if directory else f"_rels/{base}.rels"

    @staticmethod
    def _source_of(rels_key: str) -> str:
        directory, _, base = rels_key.rpartition("/")
        directory = directory.removesuffix("_rels").rstrip("/")
        name = base[: -len(".rels")]
        return f"{directory}/{name}" if directory else name

    def read(self, key: str) -> bytes:
        return self.zf.read(self.by_key[key])

    def _read_rels(self) -> None:
        for k in sorted(self.by_key):
            if not k.endswith(".rels") or k in self.removed:
                continue
            root = parse_xml(self.read(k))
            source = self._source_of(k)
            base_dir = posixpath.dirname(source)
            entries = []
            for rel in root.iter(f"{{{PKG_REL_NS}}}Relationship"):
                rid = rel.get("Id", "")
                target = rel.get("Target", "")
                rtype = rel.get("Type", "")
                external = rel.get("TargetMode", "").lower() == "external"
                entries.append((rid, rtype, target, external))
                if external:
                    hyperlink = rtype.lower().endswith("/hyperlink")
                    if hyperlink and _is_web(target):
                        if self.remove_links:
                            self._drop(k, source, rid)
                            self.web_links += 1
                        continue
                    self._drop(k, source, rid)
                    self.external += 1
                else:
                    resolved = _key(
                        target[1:] if target.startswith("/") else posixpath.join(base_dir, target)
                    )
                    resolved = posixpath.normpath(resolved)
                    if resolved in self.removed:
                        self._drop(k, source, rid)
            self.rels[k] = entries

    def _drop(self, rels_key: str, source: str, rid: str) -> None:
        self.drop_rel_ids.setdefault(rels_key, set()).add(rid)
        self.dropped_rids.setdefault(source, set()).add(rid)

    # ---------------------------------------------- field codes and actions

    def word_parts(self) -> list[str]:
        return [
            k
            for k in self.by_key
            if k.startswith("word/") and k.endswith(".xml") and "/" not in k[5:]
        ]

    def deck_parts(self) -> list[str]:
        return [k for k in self.by_key if k.startswith("ppt/") and k.endswith(".xml")]


def _fields(root: Any, remove: bool, counts: dict[str, int]) -> bool:
    """Count (and neutralize) DDE, linking and macro-button fields in one
    WordprocessingML part. Returns True if the tree changed."""
    changed = False
    instr_attr = f"{{{W_NS}}}instr"

    def hit(code: str) -> str:
        word = code.strip().split(None, 1)[0].upper() if code.strip() else ""
        if word in ("DDE", "DDEAUTO"):
            return "dde"
        if word == "MACROBUTTON":
            return "macroButtons"
        return "fieldLinks"

    stack: list[dict[str, Any]] = []
    for el in root.iter(f"{{{W_NS}}}fldSimple", f"{{{W_NS}}}fldChar", f"{{{W_NS}}}instrText"):
        tag = el.tag.rpartition("}")[2]
        if tag == "fldSimple":
            code = el.get(instr_attr, "")
            if _FIELD.match(code):
                counts[hit(code)] = counts.get(hit(code), 0) + 1
                if remove:
                    el.set(instr_attr, ' QUOTE "" ')
                    changed = True
        elif tag == "instrText":
            if stack:
                stack[-1]["els"].append(el)
        else:
            kind = el.get(f"{{{W_NS}}}fldCharType")
            if kind == "begin":
                stack.append({"els": [], "done": False})
            elif kind in ("separate", "end") and stack:
                top = stack[-1]
                if not top["done"]:
                    top["done"] = True
                    code = "".join(e.text or "" for e in top["els"])
                    if _FIELD.match(code):
                        counts[hit(code)] = counts.get(hit(code), 0) + 1
                        if remove and top["els"]:
                            top["els"][0].text = ' QUOTE "" '
                            top["els"][0].set(f"{{{XML_NS}}}space", "preserve")
                            for e in top["els"][1:]:
                                e.text = ""
                            changed = True
                if kind == "end":
                    stack.pop()
    return changed


def _program_actions(root: Any, remove: bool, counts: dict[str, int]) -> bool:
    """Hyperlink actions in a presentation that run a program, macro or file."""
    changed = False
    for el in list(root.iter()):
        if not isinstance(el.tag, str) or _local(el.tag) not in (
            "hlinkclick",
            "hlinkhover",
            "hlinkmouseover",
        ):
            continue
        if _PROGRAM_ACTION_STR.search(el.get("action", "")):
            counts["programActions"] = counts.get("programActions", 0) + 1
            if remove:
                _drop(el)
                changed = True
    return changed


def _ooxml_scan(pkg: _Package) -> dict[str, int]:
    counts: dict[str, int] = {}
    for name, n in (
        ("macros", len(pkg.macros)),
        ("macrosheets", len(pkg.macrosheets)),
        ("activeX", len([k for k in pkg.activex if not k.endswith(".rels")])),
        ("embedded", len(pkg.embeddings)),
        ("workbookLinks", len(pkg.workbook_links)),
        ("external", pkg.external),
    ):
        if n:
            counts[name] = n
    for k in pkg.word_parts():
        data = pkg.read(k)
        if b"instr" in data:
            _fields(parse_xml(data), False, counts)
    for k in pkg.deck_parts():
        data = pkg.read(k)
        if _PROGRAM_ACTION.search(data):
            _program_actions(parse_xml(data), False, counts)
    return counts


def _open_zip(path: Path) -> zipfile.ZipFile:
    try:
        return zipfile.ZipFile(path)
    except zipfile.BadZipFile:
        with path.open("rb") as fh:
            head = fh.read(8)
        if head == CFB_MAGIC:  # an encrypted Office file is a compound file, not a ZIP
            raise AdapterError("E_PASSWORD_REQUIRED") from None
        raise AdapterError("E_INPUT_CORRUPT") from None


def _ooxml_findings(path: Path) -> list[Finding]:
    with _open_zip(path) as zf:
        try:
            counts = _ooxml_scan(_Package(zf, remove_links=False))
        except (zipfile.BadZipFile, KeyError, RuntimeError, OSError):
            raise AdapterError("E_INPUT_CORRUPT") from None
    found: list[Finding] = []
    if counts.get("macros"):
        found.append(Finding(kind="macros", detail="Macros (VBA)"))
    if counts.get("macrosheets"):
        found.append(Finding(kind="macros", detail="Macros (Excel 4.0)"))
    if counts.get("macroButtons"):
        found.append(Finding(kind="macros", detail="Macro buttons"))
    if counts.get("activeX"):
        found.append(Finding(kind="activeContent", detail="ActiveX controls"))
    if counts.get("dde"):
        found.append(Finding(kind="activeContent", detail="Automatic links (DDE)"))
    if counts.get("programActions"):
        found.append(Finding(kind="activeContent", detail="Runs a program or macro"))
    if counts.get("embedded"):
        found.append(
            Finding(
                kind="embeddedFiles",
                detail=_plural(counts["embedded"], "embedded file", "embedded files"),
            )
        )
    loads = counts.get("external", 0) + counts.get("fieldLinks", 0)
    if loads:
        found.append(Finding(kind="externalLinks", detail=_outside(loads)))
    if counts.get("workbookLinks"):
        found.append(Finding(kind="externalLinks", detail="Links to other workbooks"))
    return found


def _neutralize(el: Any, attr: str) -> None:
    """Make an element that pointed at a removed relationship harmless."""
    name = _local(el.tag)
    ns = el.tag.partition("}")[0][1:] if el.tag.startswith("{") else ""
    if name in _DROP_ELEMENTS or (name == "hyperlink" and ns == SS_NS):
        victim = el
        if name == "oleobj":
            node = el.getparent()
            while node is not None and _local(node.tag) != "graphicframe":
                node = node.getparent()
            if node is not None:
                victim = node
                holder = node.getparent()
                if holder is not None and _local(holder.tag) in ("choice", "fallback"):
                    outer = holder.getparent()
                    if outer is not None and _local(outer.tag) == "alternatecontent":
                        victim = outer
        parent = victim.getparent()
        _drop(victim)
        while parent is not None and len(parent) == 0 and _local(parent.tag) in _PRUNE_EMPTY:
            up = parent.getparent()
            _drop(parent)
            parent = up
    else:
        del el.attrib[attr]


def _strip_dropped(root: Any, rids: set[str]) -> bool:
    changed = False
    for el in list(root.iter()):
        if not isinstance(el.tag, str):
            continue
        for attr, value in list(el.attrib.items()):
            if value in rids and (attr.startswith(f"{{{REL_NS}}}") or _local(attr) == "relid"):
                _neutralize(el, attr)
                changed = True
                break
    return changed


def disarm_ooxml(src: Path, dst: Path, *, remove_links: bool = False) -> dict[str, int]:
    """Copy an OOXML file without macros, ActiveX, embedded objects, links that
    load from outside the file, DDE and program-running actions. The main part's
    content type becomes the plain one (``docm`` to ``docx``). Returns counts:
    ``macros``, ``activeX``, ``embedded``, ``external``, ``workbookLinks``,
    ``dde``, ``fieldLinks``, ``macroButtons``, ``programActions``, ``links``
    (web links, only with ``remove_links``). Excel 4.0 macro sheets can't be
    removed safely and are reported as ``macrosheets`` (still present).
    Raises ``E_INPUT_CORRUPT`` for a damaged or hostile package."""
    counts: dict[str, int] = {}
    with _open_zip(src) as zf:
        try:
            pkg = _Package(zf, remove_links)
            counts.update(_ooxml_scan(pkg))
            if pkg.web_links:
                counts["links"] = pkg.web_links
            replace: dict[str, bytes] = {}
            drop = {pkg.by_key[k].filename for k in pkg.removed if k in pkg.by_key}

            # Relationship files: delete the dropped relationships.
            for rels_key, rids in pkg.drop_rel_ids.items():
                root = parse_xml(pkg.read(rels_key))
                for rel in list(root.iter(f"{{{PKG_REL_NS}}}Relationship")):
                    if rel.get("Id") in rids:
                        _drop(rel)
                replace[pkg.by_key[rels_key].filename] = _serialize(root)

            # Parts that referenced them, field codes and program actions.
            wanted = set(pkg.dropped_rids) | set(pkg.word_parts()) | set(pkg.deck_parts())
            for k in sorted(wanted):
                if k not in pkg.by_key or k in pkg.removed or k.endswith(".rels"):
                    continue
                data = pkg.read(k)
                rids = pkg.dropped_rids.get(k, set())
                is_word = k in pkg.word_parts() and b"instr" in data
                is_deck = k in pkg.deck_parts() and bool(_PROGRAM_ACTION.search(data))
                if not (rids or is_word or is_deck):
                    continue
                root = parse_xml(data)
                changed = False
                if rids:
                    changed |= _strip_dropped(root, rids)
                if is_word:
                    changed |= _fields(root, True, {})
                if is_deck:
                    changed |= _program_actions(root, True, {})
                if changed:
                    replace[pkg.by_key[k].filename] = _serialize(root)

            # Content types.
            ct_key = "[content_types].xml"
            if ct_key in pkg.by_key:
                root = parse_xml(pkg.read(ct_key))
                for el in list(root):
                    if not isinstance(el.tag, str):
                        continue
                    part = el.get("PartName")
                    if part is not None and _key(part) in pkg.removed:
                        _drop(el)
                        continue
                    ctype = el.get("ContentType", "")
                    if ctype in _MACROENABLED:
                        el.set("ContentType", _MACROENABLED[ctype])
                    elif el.tag == f"{{{CT_NS}}}Default" and ctype == _VBA_CONTENT_TYPE:
                        _drop(el)
                replace[pkg.by_key[ct_key].filename] = _serialize(root)
            _rewrite_zip(zf, pkg.infos, dst, replace, drop)
        except (zipfile.BadZipFile, KeyError, RuntimeError, OSError):
            raise AdapterError("E_INPUT_CORRUPT") from None
    return counts


def _rewrite_zip(
    zf: zipfile.ZipFile,
    infos: list[zipfile.ZipInfo],
    dst: Path,
    replace: dict[str, bytes],
    drop: set[str],
) -> None:
    """Copy the package in its original order, with fixed timestamps so the
    same input always gives the same bytes."""
    with zipfile.ZipFile(dst, "w", zipfile.ZIP_DEFLATED) as out:
        for info in infos:
            if info.filename in drop:
                continue
            fresh = zipfile.ZipInfo(info.filename, date_time=(1980, 1, 1, 0, 0, 0))
            fresh.compress_type = zipfile.ZIP_STORED if info.is_dir() else zipfile.ZIP_DEFLATED
            fresh.external_attr = info.external_attr
            if info.is_dir():
                out.writestr(fresh, b"")
            elif info.filename in replace:
                out.writestr(fresh, replace[info.filename])
            else:
                with (
                    zf.open(info) as source,
                    out.open(fresh, "w", force_zip64=info.file_size > 2**31 - 1) as sink,
                ):
                    shutil.copyfileobj(source, sink, 1024 * 1024)


# ------------------------------------------------------- SVG and HTML

_FRAME_ELEMENTS = {
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "foreignobject",
    "portal",
}
_SCRIPT_ELEMENTS = {"script", "handler", "listener"}
_LOADING_REL = re.compile(
    r"stylesheet|icon|preload|prefetch|import|modulepreload|manifest|preconnect|prerender", re.I
)
_URL_ATTRS = {
    "href",
    "src",
    "poster",
    "background",
    "data",
    "action",
    "formaction",
    "longdesc",
    "usemap",
    "codebase",
    "archive",
    "icon",
    "manifest",
    "ping",
    "srcset",
}
_NO_LOAD_ATTRS = {"action", "formaction"}
"""URL attributes that submit rather than load: only code-like schemes matter."""
_CSS_URL = re.compile(r"url\(\s*(['\"]?)(.*?)\1\s*\)", re.I | re.S)
_CSS_IMPORT = re.compile(r"@import\b[^;{}]*;?", re.I)
_CSS_ACTIVE = re.compile(r"[^;{}]*(?:expression\s*\(|-moz-binding|behavior\s*:)[^;}]*;?", re.I)
_SRCSET_EXTERNAL = re.compile(r"(?:^|[\s,])(?:[a-z][a-z0-9+.-]*:)?//", re.I)
_CODE_SCHEMES = {"javascript", "vbscript", "livescript"}


class _Markup:
    """Counts and optionally removes active content in an SVG or HTML tree."""

    def __init__(self, remove: bool, remove_links: bool) -> None:
        self.remove = remove
        self.remove_links = remove_links
        self.counts: dict[str, int] = {}

    def hit(self, kind: str, n: int = 1) -> None:
        self.counts[kind] = self.counts.get(kind, 0) + n

    # -- CSS --------------------------------------------------------------

    def css(self, text: str) -> str:
        """Count (and strip) external loads and scripts in CSS text."""
        imports = len(_CSS_IMPORT.findall(text))
        if imports:
            self.hit("external", imports)
            text = _CSS_IMPORT.sub("", text)

        def url(m: re.Match[str]) -> str:
            target = m.group(2).strip()
            low = _CONTROL.sub("", target).lower()
            if not low or low.startswith(("#", "data:image/")) or _scheme(target) == "":
                return m.group(0)
            self.hit("external")
            return "url()"

        text = _CSS_URL.sub(url, text)
        active = len(_CSS_ACTIVE.findall(text))
        if active:
            self.hit("cssActive", active)
            text = _CSS_ACTIVE.sub("", text)
        return text

    # -- one element ---------------------------------------------------------

    def attribute(self, el: Any, tag: str, name: str, value: str) -> bool:
        """Handle one attribute. Returns True to remove it."""
        local = _local(name)
        if local.startswith("on") and len(local) > 2:
            self.hit("handler")
            return True
        if local == "style" or (local not in _URL_ATTRS and "url(" in value.lower()):
            cleaned = self.css(value)
            if cleaned != value:
                if self.remove:
                    el.set(name, cleaned)
            return False
        if local not in _URL_ATTRS:
            return False
        if local == "srcset":
            if _SRCSET_EXTERNAL.search(value):
                self.hit("external")
                return True
            return False
        scheme = _scheme(value)
        low = _CONTROL.sub("", value).lower()
        if scheme in _CODE_SCHEMES:
            self.hit("jsUrl")
            return True
        if scheme == "data":
            if low.startswith("data:image/"):
                return False
            self.hit("jsUrl")
            return True
        if scheme == "" or low.startswith("#") or local in _NO_LOAD_ATTRS:
            return False
        if tag in ("a", "area"):
            if scheme in WEB_SCHEMES or scheme in ("ftp", "//"):
                if self.remove_links and scheme in WEB_SCHEMES and scheme != "mailto":
                    self.hit("links")
                    return True
                return False
            self.hit("scheme")
            return True
        if local == "ping":
            self.hit("external")
            return True
        if local in ("href", "src", "poster", "background", "data", "icon", "manifest"):
            self.hit("external")
            return True
        return False

    def element(self, el: Any, tag: str) -> bool:
        """Returns True if the whole element should go."""
        if tag in _SCRIPT_ELEMENTS:
            self.hit("script")
            return True
        if tag in _FRAME_ELEMENTS:
            self.hit("frame")
            return True
        if tag == "base":
            self.hit("base")
            return True
        if tag == "meta" and el.get("http-equiv", "").strip().lower() == "refresh":
            self.hit("refresh")
            return True
        if tag in ("animate", "set") and _local(el.get("attributeName", "")) == "href":
            self.hit("script")
            return True
        if tag == "style" and el.text:
            cleaned = self.css(el.text)
            if cleaned != el.text and self.remove:
                el.text = cleaned
        if tag == "link" and _LOADING_REL.search(el.get("rel", "")):
            href = el.get("href", "")
            if href and _scheme(href) not in ("", "data"):
                self.hit("external")
                return True
        return False

    def run(self, root: Any) -> None:
        stack = [root]
        visited = 0
        while stack:
            el = stack.pop()
            visited += 1
            if visited > MAX_NODES:
                raise AdapterError("E_LIMIT_EXCEEDED", {"limit": "elements", "max": MAX_NODES})
            if not isinstance(el.tag, str):
                # Comments: only IE conditional comments can carry live markup.
                if (
                    self.remove
                    and el.tag is _etree().Comment
                    and (el.text or "").lstrip().startswith(("[if", "<![endif"))
                ):
                    _drop(el)
                continue
            tag = _local(el.tag)
            if el is not root and self.element(el, tag):
                if self.remove:
                    _drop(el)
                continue
            for name, value in list(el.attrib.items()):
                if self.attribute(el, tag, name, value) and self.remove:
                    del el.attrib[name]
            stack.extend(reversed(list(el)))

    def stylesheet_pis(self, root: Any) -> None:
        """``<?xml-stylesheet href=…?>`` in an SVG's prolog."""
        for pi in list(root.itersiblings(preceding=True)):
            if getattr(pi, "target", None) == "xml-stylesheet":
                href = re.search(r"href\s*=\s*[\"']([^\"']*)", pi.text or "")
                if href and _scheme(href.group(1)) not in ("", "data"):
                    self.hit("external")
                    if self.remove:
                        pi.text = 'type="text/css"'  # a prolog node can't be removed in lxml


def _markup_findings_from(c: dict[str, int]) -> list[Finding]:
    found: list[Finding] = []
    if c.get("script") or c.get("handler") or c.get("jsUrl"):
        found.append(Finding(kind="activeContent", detail="JavaScript"))
    if c.get("frame"):
        found.append(Finding(kind="activeContent", detail="Embedded frames or objects"))
    if c.get("refresh"):
        found.append(Finding(kind="activeContent", detail="Automatic redirect"))
    if c.get("base"):
        found.append(Finding(kind="activeContent", detail="Changes where links point"))
    if c.get("scheme"):
        found.append(Finding(kind="activeContent", detail="Opens another program"))
    if c.get("cssActive"):
        found.append(Finding(kind="activeContent", detail="Script in styles"))
    if c.get("external"):
        found.append(Finding(kind="externalLinks", detail=_outside(c["external"])))
    return found


def _load_markup(path: Path, fmt: str) -> tuple[Any, str]:
    """(root element, doctype). SVG is strict XML; HTML is parsed leniently."""
    if fmt == "svg":
        root = parse_xml(path.read_bytes())
        return root, ""
    from lxml import html as lxml_html  # noqa: PLC0415

    parser = lxml_html.HTMLParser(
        no_network=True, remove_comments=False, remove_pis=False, huge_tree=False
    )
    text = _strip_declaration(read_text(path)) or "<p></p>"
    try:
        root = lxml_html.document_fromstring(text, parser=parser)
    except _etree().ParserError:
        raise AdapterError("E_INPUT_CORRUPT") from None
    return root, root.getroottree().docinfo.doctype or ""


def _scan_markup(path: Path, fmt: str, remove: bool, remove_links: bool) -> tuple[Any, str, dict]:
    root, doctype = _load_markup(path, fmt)
    m = _Markup(remove, remove_links)
    m.run(root)
    if fmt == "svg":
        m.stylesheet_pis(root)
    return root, doctype, m.counts


def disarm_markup(src: Path, dst: Path, fmt: str, *, remove_links: bool = False) -> dict[str, int]:
    """Write ``src`` (``svg`` or ``html``) without scripts, event handlers,
    ``javascript:`` and non-image ``data:`` URLs, frames and embedded objects,
    redirects, ``<base>``, and anything that loads from outside the file.
    ``#fragment`` and ``data:image/`` URLs and plain web links stay (web links
    go with ``remove_links``). Returns counts by kind: ``script``, ``handler``,
    ``jsUrl``, ``frame``, ``refresh``, ``base``, ``scheme``, ``cssActive``,
    ``external``, ``links``."""
    root, doctype, counts = _scan_markup(src, fmt, True, remove_links)
    etree = _etree()
    if fmt == "svg":
        dst.write_bytes(_serialize(root))
    else:
        data = etree.tostring(
            root.getroottree(), method="html", encoding="utf-8", doctype=doctype or None
        )
        dst.write_bytes(_html_with_charset(data))
    return counts


_CHARSET = re.compile(rb"(<meta\b[^>]*?charset\s*=\s*[\"']?)([A-Za-z0-9_.:-]+)", re.I)


def _html_with_charset(data: bytes) -> bytes:
    """The output is UTF-8: make any charset the page declares say so, or mark
    it with a byte order mark when it declares none."""
    if _CHARSET.search(data[:4096]):
        return _CHARSET.sub(rb"\1utf-8", data, count=1)
    try:
        data.decode("ascii")
        return data
    except UnicodeDecodeError:
        return b"\xef\xbb\xbf" + data


# ------------------------------------------------------------ public


def active_findings(path: Path, fmt: str) -> list[Finding]:
    """What a file could do when opened, for pdf, docx, xlsx, pptx (and the macro
    variants), svg and html; ``[]`` for any other format. Raises ``AdapterError``
    (``E_INPUT_CORRUPT``, ``E_PASSWORD_REQUIRED``, ``E_LIMIT_EXCEEDED``) for a file
    that can't be read."""
    fmt = fmt.lower()
    if fmt == "pdf":
        return _pdf_findings(path)
    if fmt in OOXML_FORMATS:
        return _ooxml_findings(path)
    if fmt in ("svg", "html"):
        _root, _doctype, counts = _scan_markup(path, fmt, False, False)
        return _markup_findings_from(counts)
    return []
