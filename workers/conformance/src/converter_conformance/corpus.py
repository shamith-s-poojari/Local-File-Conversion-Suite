"""Generated test PDFs. Built with pikepdf so the corpus needs no binary files
in the repository, and every file is reproducible.

Each page draws "Page N" in Helvetica, so text extraction and rendering
tests have something to find. Real-world files belong in the shared corpus
(``corpus/``), not here.
"""

from __future__ import annotations

from pathlib import Path

import pikepdf
from pikepdf import Array, Dictionary, Name, String

SENTINEL_PASSWORD = "sentinel-Pässwörd-7c1e"
"""Used as an input password; must never appear in output, stderr or work/."""


def _font(pdf: pikepdf.Pdf) -> pikepdf.Object:
    return pdf.make_indirect(
        Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica)
    )


def _page(pdf: pikepdf.Pdf, font: pikepdf.Object, number: int, size: tuple[float, float]):
    w, h = size
    text = f"BT /F1 24 Tf 72 {h - 96:.0f} Td (Page {number}) Tj ET\n"
    # A filled square near the bottom-left corner of user space, for render checks.
    mark = "0 0 0 rg 20 20 30 30 re f\n"
    contents = pdf.make_stream((text + mark).encode("ascii"))
    page = Dictionary(
        Type=Name.Page,
        MediaBox=Array([0, 0, w, h]),
        Resources=Dictionary(Font=Dictionary(F1=font)),
        Contents=contents,
    )
    return pikepdf.Page(pdf.make_indirect(page))


def text_pdf(
    path: Path,
    pages: int,
    *,
    size: tuple[float, float] = (612, 792),
    rotate: int = 0,
    crop_origin: tuple[float, float] | None = None,
    outline: bool = True,
    tagged: bool = False,
    title: str | None = None,
) -> Path:
    """A ``pages``-page PDF with one bookmark per page ("Page N")."""
    pdf = pikepdf.new()
    font = _font(pdf)
    for n in range(1, pages + 1):
        page = _page(pdf, font, n, size)
        pdf.pages.append(page)
    for p in pdf.pages:
        if rotate:
            p.Rotate = rotate
        if crop_origin is not None:
            x, y = crop_origin
            p.CropBox = Array([x, y, size[0], size[1]])
    if outline and pages <= 500:
        with pdf.open_outline() as o:
            o.root.extend(pikepdf.OutlineItem(f"Page {n}", n - 1) for n in range(1, pages + 1))
    if tagged:
        pdf.Root.StructTreeRoot = pdf.make_indirect(Dictionary(Type=Name.StructTreeRoot))
        pdf.Root.MarkInfo = Dictionary(Marked=True)
    if title is not None:
        pdf.docinfo[Name.Title] = title
    pdf.save(path, deterministic_id=True)
    return path


def heavy_pdf(path: Path, pages: int = 40, side: int = 1448) -> Path:
    """Pages with a ~2 MB incompressible image each, so whole-file saves take
    long enough to cancel. Deterministic: the bytes come from a seeded PRNG."""
    import random  # noqa: PLC0415

    rng = random.Random(7)  # noqa: S311 - test data, not security
    pdf = pikepdf.new()
    font = _font(pdf)
    for n in range(1, pages + 1):
        page = _page(pdf, font, n, (612, 792))
        image = pdf.make_stream(
            rng.randbytes(side * side),
            Type=Name.XObject,
            Subtype=Name.Image,
            Width=side,
            Height=side,
            ColorSpace=Name.DeviceGray,
            BitsPerComponent=8,
        )
        page.Resources.XObject = Dictionary(Im0=image)
        page.contents_add(pdf.make_stream(b"q 200 0 0 200 300 300 cm /Im0 Do Q\n"))
        pdf.pages.append(page)
    pdf.save(path, deterministic_id=True, compress_streams=False)
    return path


def form_pdf(path: Path, field_name: str = "name", value: str = "Ada") -> Path:
    """One page with one text field."""
    pdf = pikepdf.new()
    font = _font(pdf)
    page = _page(pdf, font, 1, (612, 792))
    pdf.pages.append(page)
    widget = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Widget,
            FT=Name.Tx,
            T=String(field_name),
            V=String(value),
            Rect=Array([72, 600, 300, 630]),
            DA=String("/Helv 12 Tf 0 g"),
            P=pdf.pages[0].obj,
        )
    )
    pdf.pages[0].Annots = Array([widget])
    pdf.Root.AcroForm = Dictionary(
        Fields=Array([widget]),
        DA=String("/Helv 12 Tf 0 g"),
        DR=Dictionary(Font=Dictionary(Helv=font)),
        NeedAppearances=True,
    )
    pdf.save(path, deterministic_id=True)
    return path


def signed_pdf(path: Path) -> Path:
    """Looks signed to structural checks: a /Sig field with a value. Not a real signature."""
    pdf = pikepdf.new()
    pdf.pages.append(_page(pdf, _font(pdf), 1, (612, 792)))
    sig = pdf.make_indirect(
        Dictionary(Type=Name.Sig, Filter=Name("/Adobe.PPKLite"), Contents=String(b"\0" * 16))
    )
    field = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Widget,
            FT=Name.Sig,
            T=String("Signature1"),
            V=sig,
            Rect=Array([0, 0, 0, 0]),
            P=pdf.pages[0].obj,
        )
    )
    pdf.pages[0].Annots = Array([field])
    pdf.Root.AcroForm = Dictionary(Fields=Array([field]), SigFlags=3)
    pdf.save(path, deterministic_id=True)
    return path


def active_pdf(path: Path) -> Path:
    """One page with an OpenAction JavaScript action."""
    pdf = pikepdf.new()
    pdf.pages.append(_page(pdf, _font(pdf), 1, (612, 792)))
    pdf.Root.OpenAction = Dictionary(S=Name.JavaScript, JS=String("app.alert(1)"))
    pdf.save(path, deterministic_id=True)
    return path


def encrypted_pdf(
    path: Path, pages: int = 2, *, user: str = SENTINEL_PASSWORD, owner: str = "owner-pw"
) -> Path:
    """AES-256. With ``user=""`` it is permissions-only (opens without a password)."""
    tmp = path.with_suffix(".plain.pdf")
    text_pdf(tmp, pages)
    with pikepdf.open(tmp) as pdf:
        pdf.save(
            path,
            encryption=pikepdf.Encryption(
                owner=owner,
                user=user,
                R=6,
                allow=pikepdf.Permissions(extract=False, modify_other=False),
            ),
        )
    tmp.unlink()
    return path


def corrupt_file(path: Path) -> Path:
    path.write_bytes(b"this is not a PDF file\n" * 10)
    return path


def truncated_pdf(path: Path, pages: int = 3) -> Path:
    """A valid PDF cut in half: no trailer or cross-reference table."""
    full = path.with_suffix(".full.pdf")
    text_pdf(full, pages)
    data = full.read_bytes()
    full.unlink()
    path.write_bytes(data[: len(data) // 2])
    return path


def page_texts(path: Path, password: str = "") -> list[bytes]:
    """Decoded content of each page, for equality checks that ignore object layout."""
    with pikepdf.open(path, password=password) as pdf:
        out = []
        for page in pdf.pages:
            page.contents_coalesce()
            out.append(page.Contents.read_bytes())
        return out


BUILDERS = {
    "text-1": lambda p: text_pdf(p, 1),
    "text-3": lambda p: text_pdf(p, 3),
    "text-5": lambda p: text_pdf(p, 5),
    "text-10": lambda p: text_pdf(p, 10),
    "rotated-90": lambda p: text_pdf(p, 2, rotate=90),
    "landscape": lambda p: text_pdf(p, 2, size=(792, 612)),
    "tagged": lambda p: text_pdf(p, 2, tagged=True),
    "form": form_pdf,
    "signed": signed_pdf,
    "active": active_pdf,
    "encrypted": encrypted_pdf,
    "permissions-only": lambda p: encrypted_pdf(p, user=""),
    "corrupt": corrupt_file,
    "truncated": truncated_pdf,
    "big": lambda p: text_pdf(p, 4000, outline=False),
    "heavy": heavy_pdf,
}
"""Named corpus entries. Adapter conformance cases refer to these names."""


def build(name: str, directory: Path) -> Path:
    """Build corpus file ``name`` in ``directory`` (cached by name). Names
    that aren't PDFs come from ``files`` (images, audio, video, documents)."""
    if name not in BUILDERS:
        from . import files  # noqa: PLC0415

        return files.build(name, directory)
    path = directory / f"{name}.pdf"
    if not path.exists():
        directory.mkdir(parents=True, exist_ok=True)
        BUILDERS[name](path)
    return path
