"""ConversionAdapter fills OutputFile.source for per-input outputs (ADR 0005)."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from converter_sdk import JobContext, OutputFile
from converter_sdk.convert import ConversionAdapter


def make_ctx(tmp_path: Path, n: int) -> JobContext:
    return JobContext(
        job_id="j_test",
        job_dir=tmp_path,
        inputs=[tmp_path / "in" / f"{i}.png" for i in range(n)],
        secrets=None,
        _send=lambda _m: None,
        _cancel=__import__("threading").Event(),
    )


class OnePerInput(ConversionAdapter):
    adapter_id = "one-per-input"

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        return [
            OutputFile(ctx.output_path(self.output_name(params, i, "pdf")))
            for i in range(len(ctx.inputs))
        ]


class Pages(ConversionAdapter):
    adapter_id = "pages"

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        out = []
        for i in reversed(range(len(ctx.inputs))):
            for page in (1, 2):
                out.append(
                    OutputFile(ctx.output_path(self.output_name(params, i, "png", f"-{page}")))
                )
        return out


class Combined(ConversionAdapter):
    adapter_id = "combined"

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        return [OutputFile(ctx.output_path("combined.pdf"))]


class Explicit(ConversionAdapter):
    adapter_id = "explicit"

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        return [self.output_file(ctx.output_path("x.pdf"), 1)]


def sources(outputs: list[OutputFile]) -> list[int | None]:
    return [o.source for o in outputs]


def test_one_output_per_input(tmp_path: Path) -> None:
    ctx = make_ctx(tmp_path, 3)
    out = OnePerInput().execute_conversion(ctx, {"fileTitles": ["a", "b", "c"]})
    assert sources(out) == [0, 1, 2]


def test_single_input_owns_everything(tmp_path: Path) -> None:
    ctx = make_ctx(tmp_path, 1)
    out = Pages().execute_conversion(ctx, {"fileTitles": ["a"]})
    assert sources(out) == [0, 0]


def test_several_outputs_matched_by_title(tmp_path: Path) -> None:
    ctx = make_ctx(tmp_path, 3)
    out = Pages().execute_conversion(ctx, {"fileTitles": ["a", "ab", "c"]})
    assert sources(out) == [2, 2, 1, 1, 0, 0]


def test_duplicate_titles_stay_unknown(tmp_path: Path) -> None:
    ctx = make_ctx(tmp_path, 2)
    out = Pages().execute_conversion(ctx, {"fileTitles": ["a", "a"]})
    assert sources(out) == [None, None, None, None]


def test_combined_output_has_no_source(tmp_path: Path) -> None:
    ctx = make_ctx(tmp_path, 2)
    out = Combined().execute_conversion(ctx, {"fileTitles": ["a", "b"]})
    assert sources(out) == [None]


def test_explicit_source_is_kept(tmp_path: Path) -> None:
    ctx = make_ctx(tmp_path, 2)
    out = Explicit().execute_conversion(ctx, {})
    assert sources(out) == [1]
