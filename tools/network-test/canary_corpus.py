"""Builds PDFs whose every outward reference points at a local canary listener.

Usage: python -I canary_corpus.py OUT_DIR PORT

Each file names the canary in a different way a PDF can refer to the
outside world. None of them may cause a connection: actions and scripts
are never run, remote references are never resolved (spec 14.5, item 6).
"""

import sys
from pathlib import Path

import pikepdf
from pikepdf import Array, Dictionary, Name, String


def canary(port: int, what: str) -> str:
    return f"http://127.0.0.1:{port}/canary/{what}"


def base(pages: int = 2) -> pikepdf.Pdf:
    pdf = pikepdf.new()
    font = pdf.make_indirect(
        Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica)
    )
    for n in range(1, pages + 1):
        content = pdf.make_stream(f"BT /F1 18 Tf 72 700 Td (canary page {n}) Tj ET".encode())
        page = Dictionary(
            Type=Name.Page,
            MediaBox=[0, 0, 612, 792],
            Resources=Dictionary(Font=Dictionary(F1=font)),
            Contents=content,
        )
        pdf.pages.append(pikepdf.Page(pdf.make_indirect(page)))
    return pdf


def build(out: Path, port: int) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    made = []

    def save(name: str, pdf: pikepdf.Pdf) -> None:
        path = out / f"{name}.pdf"
        pdf.save(path)
        made.append(path)

    # Link annotation with a URI action.
    pdf = base()
    link = pdf.make_indirect(
        Dictionary(
            Type=Name.Annot,
            Subtype=Name.Link,
            Rect=[72, 690, 300, 720],
            A=Dictionary(S=Name.URI, URI=String(canary(port, "link"))),
        )
    )
    pdf.pages[0].Annots = Array([link])
    save("link", pdf)

    # Actions that run on open, and on page open; a form that submits.
    pdf = base()
    pdf.Root.OpenAction = Dictionary(S=Name.URI, URI=String(canary(port, "open")))
    pdf.pages[0].AA = Dictionary(
        O=Dictionary(S=Name.SubmitForm, F=Dictionary(FS=Name.URL, F=String(canary(port, "submit"))))
    )
    pdf.Root.AA = Dictionary(
        WC=Dictionary(
            S=Name.GoToR,
            F=Dictionary(FS=Name.URL, F=String(canary(port, "gotor"))),
            D=Array([0, Name.Fit]),
        )
    )
    save("actions", pdf)

    # Document JavaScript that would fetch the canary if it were ever run.
    pdf = base()
    js = pdf.make_indirect(
        Dictionary(S=Name.JavaScript, JS=String(f"app.launchURL('{canary(port, 'js')}', true);"))
    )
    pdf.Root.Names = Dictionary(JavaScript=Dictionary(Names=Array([String("init"), js])))
    save("javascript", pdf)

    # An attachment, XMP metadata and an external stream naming the canary.
    pdf = base()
    pdf.attachments["notes.html"] = pikepdf.AttachedFileSpec(
        pdf, f'<img src="{canary(port, "attachment")}">'.encode(), mime_type="text/html"
    )
    with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:
        meta["dc:source"] = canary(port, "xmp")
    remote = pdf.make_stream(
        b"",
        Type=Name.XObject,
        Subtype=Name.Image,
        Width=1,
        Height=1,
        ColorSpace=Name.DeviceGray,
        BitsPerComponent=8,
        F=Dictionary(FS=Name.URL, F=String(canary(port, "external-stream"))),
    )
    pdf.pages[0].Resources.XObject = Dictionary(Remote=remote)
    pdf.pages[0].contents_add(pdf.make_stream(b"q 10 0 0 10 400 400 cm /Remote Do Q"))
    save("embedded", pdf)

    return made


if __name__ == "__main__":
    for p in build(Path(sys.argv[1]), int(sys.argv[2])):
        print(p)
