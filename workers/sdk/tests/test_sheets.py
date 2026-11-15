"""Contact-sheet grid helpers (converter_sdk.sheets)."""

from __future__ import annotations

import pytest
from converter_sdk import sheets
from converter_sdk.images import MAX_PIXELS
from PIL import Image


def cell(color, size=(40, 30), caption="x"):
    return Image.new("RGB", size, color), caption


def test_fit_on_canvas_centres_on_white() -> None:
    out = sheets.fit_on_canvas(Image.new("RGB", (40, 10), (255, 0, 0)), (20, 20))
    assert out.size == (20, 20)
    assert out.getpixel((10, 10)) == (255, 0, 0)
    assert out.getpixel((10, 1)) == (255, 255, 255)


def test_grid_places_cells_row_by_row() -> None:
    colors = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (0, 255, 255)]
    cells = [cell(c, (50, 50), None) for c in colors]
    s = sheets.contact_sheet(cells, columns=2, cell_px=50, margin=10)
    m = sheets._Metrics(5, 2, 50, 1.0, False, False, 10)
    assert s.size == (m.width, m.height) and m.rows == 3
    for i, c in enumerate(colors):
        row, col = divmod(i, 2)
        x = 10 + col * (50 + m.gap) + 25
        y = 10 + row * m.step_y + 25
        assert s.getpixel((x, y)) == c
    assert s.getpixel((5, 5)) == (255, 255, 255)


def test_captions_add_height_and_ink() -> None:
    plain = sheets.contact_sheet([cell("red", caption=None)], columns=1, cell_px=100)
    labelled = sheets.contact_sheet([cell("red", caption="holiday.jpg")], columns=1, cell_px=100)
    assert labelled.height > plain.height
    below = labelled.crop((0, plain.height - 10, labelled.width, labelled.height))
    assert below.convert("L").getextrema()[0] < 128  # dark text pixels


def test_title_adds_a_band() -> None:
    a = sheets.contact_sheet([cell("red")], columns=1, cell_px=100)
    b = sheets.contact_sheet([cell("red")], columns=1, cell_px=100, title="Trip")
    assert b.height > a.height
    assert b.crop((0, 0, b.width, 20)).convert("L").getextrema()[0] < 128


def test_long_caption_is_cut_to_the_cell() -> None:
    s = sheets.contact_sheet(
        [cell("white", caption="a" * 200), cell("white", caption="b")], columns=2, cell_px=80
    )
    assert s.width < 400


def test_transparency_is_filled_with_the_background() -> None:
    clear = Image.new("RGBA", (30, 30), (0, 0, 0, 0))
    s = sheets.contact_sheet(
        [(clear, None)], columns=1, cell_px=30, margin=5, background=(10, 20, 30)
    )
    assert s.getpixel((20, 20)) == (10, 20, 30)


def test_aspect_and_min_size() -> None:
    s = sheets.contact_sheet(
        [cell("red", (160, 90), None)], columns=1, cell_px=160, aspect=16 / 9, margin=0
    )
    assert s.size == (160, 90)
    s = sheets.contact_sheet([cell("red")], columns=1, cell_px=50, min_size=(600, 800))
    assert s.size == (600, 800)


def test_stays_under_the_pixel_limit(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sheets, "MAX_PIXELS", 40_000)
    s = sheets.contact_sheet([cell("red", caption="a")] * 9, columns=3, cell_px=400)
    assert s.width * s.height <= 40_000
    assert MAX_PIXELS == 200_000_000


def test_sheet_pages_and_rows() -> None:
    assert sheets.sheet_pages(list(range(5)), 2) == [[0, 1], [2, 3], [4]]
    assert sheets.sheet_pages([], 4) == [[]]
    assert sheets.sheet_pages([1], 0) == [[1]]
    cell_px = sheets.cell_px_for_width(1240, 4)
    rows = sheets.rows_per_page(1754, cell_px)
    assert rows >= 3
    m = sheets._Metrics(rows * 4, 4, cell_px, 1.0, True, False, None)
    assert m.width <= 1240 and m.height <= 1754
