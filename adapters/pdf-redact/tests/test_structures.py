"""PRIV-1 and PRIV-7: text under a box must also leave the places outside the
page content (marked-content ActualText and Alt, the structure tree, outlines,
page labels, named destinations, article threads, optional-content states)."""

from __future__ import annotations

import io
import json
import subprocess
import sys
from pathlib import Path

import pikepdf
import pymupdf
import pytest
from converter_conformance.harness import run_in_process
from pikepdf import Array, Dictionary, Name, String

ADAPTER = Path(__file__).resolve().parents[1]
VERIFY = ADAPTER / "verify.py"
WORD = "Zorblax7731"
KEEP = "Visible-Keeper"
OVERVIEW = "Overview"


def _pdf_with_places_for_the_word(path: Path, *, tagged: bool = True) -> Path:
    """A one-page file whose page shows WORD and KEEP, and that repeats WORD in
    every place a text-only redactor forgets."""
    pdf = pikepdf.new()
    font = pdf.make_indirect(
        Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica)
    )
    content = (
        f"/Span <</ActualText ({WORD})>> BDC BT /F1 12 Tf 72 700 Td ({WORD}) Tj ET EMC\n"
        f"/Figure <</Alt (photo of {WORD})>> BDC EMC\n"
        f"/P <</MCID 0>> BDC BT /F1 12 Tf 72 650 Td ({KEEP}) Tj ET EMC\n"
    ).encode("latin-1")
    page_dict = pdf.make_indirect(
        Dictionary(
            Type=Name.Page,
            MediaBox=[0, 0, 612, 792],
            Resources=Dictionary(Font=Dictionary(F1=font)),
            Contents=pdf.make_stream(content),
            StructParents=0,
        )
    )
    pdf.pages.append(pikepdf.Page(page_dict))
    root = pdf.Root
    if tagged:
        elem = pdf.make_indirect(
            Dictionary(
                Type=Name.StructElem,
                S=Name.Span,
                Pg=page_dict,
                K=0,
                ActualText=String(WORD),
                Alt=String(f"picture of {WORD}"),
            )
        )
        root.StructTreeRoot = pdf.make_indirect(
            Dictionary(Type=Name.StructTreeRoot, K=Array([elem]))
        )
        root.MarkInfo = Dictionary(Marked=True)
    first = pdf.make_indirect(
        Dictionary(Title=String(f"Report on {WORD}"), Dest=[page_dict, Name.Fit])
    )
    second = pdf.make_indirect(Dictionary(Title=String(OVERVIEW), Dest=[page_dict, Name.Fit]))
    outlines = pdf.make_indirect(Dictionary(Type=Name.Outlines, Count=2))
    outlines.First, outlines.Last = first, second
    first.Parent = second.Parent = outlines
    first.Next, second.Prev = second, first
    root.Outlines = outlines
    root.PageLabels = Dictionary(Nums=Array([0, Dictionary(S=Name.D, P=String(f"{WORD}-"))]))
    root.Dests = Dictionary({f"/sec{WORD}": Array([page_dict, Name.Fit])})
    root.Names = Dictionary(
        Dests=Dictionary(Names=Array([String(f"chapter-{WORD}"), Array([page_dict, Name.Fit])]))
    )
    bead = pdf.make_indirect(Dictionary(Type=Name.Bead, P=page_dict, R=[72, 690, 300, 710]))
    bead.N = bead.V = bead
    thread = Dictionary(F=bead, I=Dictionary(Title=String(f"{WORD} story")))
    root.Threads = Array([pdf.make_indirect(thread)])
    pdf.save(path)
    return path


def _box_over_word(path: Path) -> dict:
    with pymupdf.open(path) as doc:
        rect = doc[0].search_for(WORD)[0]
        w, h = doc[0].rect.width, doc[0].rect.height
    return {
        "page": 1,
        "x0": (rect.x0 - 2) / w,
        "y0": (rect.y0 - 2) / h,
        "x1": (rect.x1 + 2) / w,
        "y1": (rect.y1 + 2) / h,
    }


def _raw(path: Path) -> bytes:
    with pikepdf.open(path) as pdf:
        buf = io.BytesIO()
        pdf.save(
            buf,
            stream_decode_level=pikepdf.StreamDecodeLevel.all,
            object_stream_mode=pikepdf.ObjectStreamMode.disable,
            compress_streams=False,
        )
    data = buf.getvalue().lower()
    return data


def _mentions(path: Path, word: str) -> bool:
    data = _raw(path)
    needle = word.casefold()
    return any(
        needle.encode(enc) in data or needle.encode(enc).hex().encode() in data
        for enc in ("latin-1", "utf-16-be")
    )


def _redact(tmp_path: Path, src: Path, **params):
    run = run_in_process(ADAPTER, tmp_path / "job", [src], params)
    assert run.errors == [], run.errors
    return run


def test_a_box_alone_removes_the_text_from_every_place_it_was_copied_to(tmp_path) -> None:
    src = _pdf_with_places_for_the_word(tmp_path / "in.pdf")
    assert _mentions(src, WORD)
    run = _redact(tmp_path, src, boxes=[_box_over_word(src)])
    out = run.outputs[0]
    assert not _mentions(out, WORD)
    assert _mentions(out, KEEP) or KEEP in "".join(p.get_text() for p in pymupdf.open(out))
    with pikepdf.open(out) as pdf:
        assert "/StructTreeRoot" not in pdf.Root and "/MarkInfo" not in pdf.Root
        assert "/Threads" not in pdf.Root
    with pymupdf.open(out) as doc:
        assert [t[1] for t in doc.get_toc()] == [OVERVIEW]
    assert "TAGS_DROPPED" in run.warnings


def test_an_untagged_file_does_not_get_the_tags_warning(tmp_path) -> None:
    src = _pdf_with_places_for_the_word(tmp_path / "in.pdf", tagged=False)
    run = _redact(tmp_path, src, boxes=[_box_over_word(src)])
    assert not _mentions(run.outputs[0], WORD)
    assert "TAGS_DROPPED" not in run.warnings


def test_text_in_a_content_stream_property_list_is_removed(tmp_path) -> None:
    """The property list sits in a resource, not inline: ``/Span /MC0 BDC``."""
    src = tmp_path / "in.pdf"
    _pdf_with_places_for_the_word(src, tagged=False)
    with pikepdf.open(src, allow_overwriting_input=True) as pdf:
        page = pdf.pages[0]
        page.Resources.Properties = Dictionary(MC0=Dictionary(ActualText=String(WORD)))
        page.Contents = pdf.make_stream(
            f"/Span /MC0 BDC BT /F1 12 Tf 72 700 Td ({WORD}) Tj ET EMC\n"
            f"BT /F1 12 Tf 72 650 Td ({KEEP}) Tj ET\n".encode()
        )
        pdf.save(src)
    run = _redact(tmp_path, src, boxes=[_box_over_word(src)])
    assert not _mentions(run.outputs[0], WORD)


def _verify(tmp_path: Path, name: str, **request) -> dict:
    payload = {"output": name, "expectedPages": 1, "fill": [0, 0, 0], **request}
    done = subprocess.run(  # noqa: S603 - our interpreter and our verifier
        [sys.executable, "-I", str(VERIFY)],
        input=json.dumps(payload).encode(),
        capture_output=True,
        cwd=tmp_path,
        timeout=120,
        check=True,
    )
    return json.loads(done.stdout)


def _failed(report: dict) -> dict[str, dict]:
    return {c["id"]: c for c in report["checks"] if not c["passed"]}


@pytest.mark.parametrize(
    "place",
    ["actualText", "outline", "pageLabel", "namedDest", "nameTree", "thread", "struct"],
)
def test_the_verifier_looks_for_the_captured_text_outside_the_page(tmp_path, place) -> None:
    """Independent of the adapter: hand the verifier a file that still has the
    word in one place only, and it must fail."""
    src = tmp_path / "in.pdf"
    _pdf_with_places_for_the_word(src)
    with pikepdf.open(src, allow_overwriting_input=True) as pdf:
        root = pdf.Root
        # Clean everything first, then put the word back in one place.
        page = pdf.pages[0]
        page.Contents = pdf.make_stream(
            b"/Span <</ActualText (x)>> BDC BT ET EMC"
            if place != "actualText"
            else f"/Span <</ActualText ({WORD})>> BDC BT ET EMC".encode()
        )
        del root.StructTreeRoot
        del root.MarkInfo
        root.Outlines.First.Title = String(OVERVIEW if place != "outline" else f"a {WORD}")
        root.PageLabels = Dictionary(
            Nums=Array([0, Dictionary(S=Name.D, P=String(WORD if place == "pageLabel" else "p"))])
        )
        root.Dests = Dictionary(
            {f"/{WORD if place == 'namedDest' else 'plain'}": Array([page.obj, Name.Fit])}
        )
        root.Names = Dictionary(
            Dests=Dictionary(
                Names=Array(
                    [String(WORD if place == "nameTree" else "plain"), Array([page.obj, Name.Fit])]
                )
            )
        )
        root.Threads = Array(
            [Dictionary(I=Dictionary(Title=String(WORD if place == "thread" else "t")))]
        )
        if place == "struct":
            root.StructTreeRoot = pdf.make_indirect(
                Dictionary(Type=Name.StructTreeRoot, K=Dictionary(S=Name.P, Alt=String(WORD)))
            )
        pdf.save(tmp_path / "out.pdf")
    report = _verify(tmp_path, "out.pdf", captured=[WORD.lower()])
    assert "V10" in _failed(report), report


def test_the_verifier_passes_a_file_without_them(tmp_path) -> None:
    src = tmp_path / "in.pdf"
    _pdf_with_places_for_the_word(src, tagged=False)
    with pikepdf.open(src, allow_overwriting_input=True) as pdf:
        pdf.Root.Outlines.First.Title = String(OVERVIEW)
        del pdf.Root.PageLabels, pdf.Root.Dests, pdf.Root.Names, pdf.Root.Threads
        pdf.pages[0].Contents = pdf.make_stream(b"BT ET")
        pdf.save(tmp_path / "out.pdf")
    report = _verify(tmp_path, "out.pdf", captured=[WORD.lower()])
    assert "V10" not in _failed(report), report


def test_the_verifier_names_the_places_not_the_text(tmp_path) -> None:
    src = _pdf_with_places_for_the_word(tmp_path / "in.pdf")
    report = _verify(tmp_path, "in.pdf", captured=[WORD.lower()])
    check = _failed(report)["V10"]
    assert WORD.lower() not in json.dumps(report).lower()
    assert set(check["places"]) >= {"actualText", "outline", "pageLabel", "destination", "thread"}
    del src


# ------------------------------------------------------------ optional content


def test_removing_hidden_layers_follows_the_auto_state_arrays() -> None:
    """A group that ``/AS`` switches off for viewing is hidden although it is
    not in ``/OFF``; its content must go, and ``/AS`` with it."""
    from converter_sdk.pdf import remove_hidden_layers  # noqa: PLC0415

    pdf = pikepdf.new()
    font = pdf.make_indirect(
        Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica)
    )
    layer = pdf.make_indirect(
        Dictionary(
            Type=Name.OCG,
            Name=String("secret layer"),
            Usage=Dictionary(View=Dictionary(ViewState=Name.OFF)),
        )
    )
    pdf.Root.OCProperties = Dictionary(
        OCGs=Array([layer]),
        D=Dictionary(
            ON=Array([layer]),
            AS=Array(
                [Dictionary(Event=Name.View, Category=Array([Name.View]), OCGs=Array([layer]))]
            ),
        ),
    )
    content = (
        f"/OC /L1 BDC BT /F1 12 Tf 72 700 Td ({WORD}) Tj ET EMC\n"
        f"BT /F1 12 Tf 72 650 Td ({KEEP}) Tj ET\n"
    )
    pdf.pages.append(
        pikepdf.Page(
            pdf.make_indirect(
                Dictionary(
                    Type=Name.Page,
                    MediaBox=[0, 0, 612, 792],
                    Resources=Dictionary(Font=Dictionary(F1=font), Properties=Dictionary(L1=layer)),
                    Contents=pdf.make_stream(content.encode()),
                )
            )
        )
    )
    assert remove_hidden_layers(pdf)
    shown = b"".join(
        bytes(op[0]) for op, name in pikepdf.parse_content_stream(pdf.pages[0]) if str(name) == "Tj"
    )
    assert WORD.encode() not in shown and KEEP.encode() in shown
    assert "/OCProperties" not in pdf.Root


def test_the_verifier_rejects_leftover_auto_state_arrays(tmp_path) -> None:
    pdf = pikepdf.new()
    pdf.add_blank_page()
    pdf.Root.OCProperties = Dictionary(OCGs=Array([]), D=Dictionary(AS=Array([])))
    pdf.save(tmp_path / "out.pdf")
    assert "V5" in _failed(_verify(tmp_path, "out.pdf"))
