"""Spec section 8.4: every shipping adapter passes the same checks.

Cases come from ``adapters/<id>/tests/conformance.yaml``; see this kit's README.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path
from typing import Any

import pytest
import yaml
from converter_conformance import corpus, files
from converter_conformance import validate as output_check
from converter_conformance.harness import (
    WorkerProcess,
    load_adapter_class,
    make_job_dir,
    run_job,
)

ROOT = Path(__file__).resolve().parents[3]
ADAPTERS = ROOT / "adapters"
SCHEMA = ROOT / "packages" / "protocol" / "schema" / "protocol.schema.json"
CANCEL_LIMIT_S = 2.0
MEMORY_FACTOR = 1.5
STRUCTURED_INPUT_ERRORS = {"E_INPUT_CORRUPT", "E_PASSWORD_REQUIRED", "E_LIMIT_EXCEEDED"}


def _shipping() -> list[tuple[str, dict[str, Any], dict[str, Any]]]:
    found = []
    for manifest_path in sorted(ADAPTERS.glob("*/manifest.yaml")):
        manifest = yaml.safe_load(manifest_path.read_text("utf-8"))
        if manifest.get("devOnly"):
            continue
        cases_path = manifest_path.parent / "tests" / "conformance.yaml"
        cases = yaml.safe_load(cases_path.read_text("utf-8")) if cases_path.exists() else {}
        found.append((manifest["id"], manifest, cases))
    return found


ADAPTER_LIST = _shipping()
IDS = [a[0] for a in ADAPTER_LIST]
CATALOG = set(json.loads(SCHEMA.read_text("utf-8"))["definitions"]["WarningCode"]["enum"])


@pytest.fixture(scope="session")
def corpus_dir(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return tmp_path_factory.mktemp("corpus")


def _adapter(adapter_id: str) -> Path:
    return ADAPTERS / adapter_id / "adapter.py"


def _files(job_dir: Path) -> set[str]:
    return {p.relative_to(job_dir).as_posix() for p in job_dir.rglob("*") if p.is_file()}


def _job(tmp_path: Path, corpus_dir: Path, names: list[str]) -> tuple[Path, list[str]]:
    return make_job_dir(tmp_path / "job", [corpus.build(n, corpus_dir) for n in names])


def _replace_first(inputs: list[str], name: str) -> list[str]:
    return [name, *inputs[1:]]


def _output_format(manifest: dict[str, Any], params: dict[str, Any], path: str) -> str:
    """The format an output must be: the target, the manifest's, or for a
    tool that keeps each file's format (ADR 0004), its own extension."""
    if manifest["kind"] == "conversion":
        return str(params["to"])
    fmt = manifest["outputs"]["format"]
    if fmt == "chosen":
        return str(params["to"])
    if fmt == "same":
        return Path(path).suffix.lstrip(".")
    return fmt


def _reads_pdf(manifest: dict[str, Any]) -> bool:
    if manifest["kind"] == "conversion":
        return any("pdf" in e["from"] for e in manifest["edges"])
    return "pdf" in manifest["inputs"]["formats"]


def _needs_missing_engine(names: list[str], manifest: dict[str, Any]) -> str | None:
    env = files.engine_env()
    if "ffmpeg" in manifest["requires"] and "LFCS_BIN_FFMPEG" not in env:
        return "ffmpeg"
    if "libreoffice" in manifest["requires"] and "LFCS_BIN_SOFFICE" not in env:
        return "libreoffice"
    if (
        any(files.needs_ffmpeg(n) for n in names if n in files.BUILDERS)
        and "LFCS_BIN_FFMPEG" not in env
    ):
        return "ffmpeg"
    if any(files.needs_soffice(n) for n in names) and "LFCS_BIN_SOFFICE" not in env:
        return "libreoffice"
    return None


def _skip_without_engines(names: list[str], manifest: dict[str, Any]) -> None:
    if missing := _needs_missing_engine(names, manifest):
        pytest.skip(f"{missing} isn't installed")


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_has_cases_readme_and_matching_class(adapter_id, manifest, cases) -> None:
    folder = ADAPTERS / adapter_id
    assert cases.get("cases"), "tests/conformance.yaml needs at least one case"
    assert {"negative", "cancel"} <= cases.keys()
    assert (folder / "README.md").is_file()
    cls = load_adapter_class(folder)
    assert cls.adapter_id == manifest["id"] == folder.name
    if manifest["kind"] == "conversion":
        declared = {(f, e["to"]): e["tier"] for e in manifest["edges"] for f in e["from"]}
        assert declared == cls.EDGES, "manifest edges differ from the adapter's EDGES"
    else:
        assert cls.fidelity == manifest["fidelity"]


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_cases_produce_valid_contained_outputs(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    for n, case in enumerate(cases["cases"]):
        _skip_without_engines(case["inputs"], manifest)
        job_dir, inputs = _job(tmp_path / str(n), corpus_dir, case["inputs"])
        before = _files(job_dir)
        r = run_job(_adapter(adapter_id), job_dir, inputs, case["params"], case.get("secrets"))
        result = r.last("executeResult")
        seen = [(m.type, getattr(m, "code", None)) for m in r.messages]
        assert result is not None, f"{case['name']}: {seen}"

        # Containment: everything new is under out/ or work/.
        new = _files(job_dir) - before
        assert all(p.startswith(("out/", "work/")) for p in new), sorted(new)

        # Validity, page counts and output counts.
        expect = case.get("expect", {})
        password = expect.get("password", "")
        pages = []
        for out in result.outputs:
            fmt = _output_format(manifest, case["params"], out.path)
            try:
                count = output_check.check(job_dir / out.path, fmt, password)
            except Exception as e:  # noqa: BLE001 - reported with the case name
                pytest.fail(f"{case['name']}: {out.path} isn't a valid {fmt}: {e}")
            if count is not None:
                pages.append(count)
                assert out.pages == count
        if "pages" in expect:
            assert pages == expect["pages"], case["name"]
        if "outputs" in expect:
            assert len(result.outputs) == expect["outputs"], case["name"]

        # Warnings: in the catalog and declared by the manifest.
        for w in r.of("warning"):
            assert w.code in CATALOG, w.code
            assert w.code in manifest["fidelityNotes"], f"{w.code} not in fidelityNotes"

        # Memory: the estimate must bound reality (spec 8.4), where measurable.
        estimate = r.last("estimateResult")
        if r.peak_rss_bytes is not None:
            assert r.peak_rss_bytes <= MEMORY_FACTOR * estimate.ramBytes, (
                f"{case['name']}: peak {r.peak_rss_bytes >> 20} MB, "
                f"estimate {estimate.ramBytes >> 20} MB"
            )


@pytest.mark.parametrize("bad", ["corrupt", "truncated"])
@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_bad_input_gives_structured_error(
    adapter_id, manifest, cases, bad, tmp_path: Path, corpus_dir: Path
) -> None:
    negative = cases["negative"]
    if "bad" in negative:
        index = 0 if bad == "corrupt" else 1
        if index >= len(negative["bad"]):
            pytest.skip("no truncated input for this format")
        inputs = _replace_first(
            negative.get("inputs", cases["cases"][0]["inputs"]), negative["bad"][index]
        )
    else:
        inputs = _replace_first(cases["cases"][0]["inputs"], bad)
    _skip_without_engines(inputs, manifest)
    job_dir, rel = _job(tmp_path, corpus_dir, inputs)
    r = run_job(
        _adapter(adapter_id),
        job_dir,
        rel,
        cases["negative"]["params"],
        cases["negative"].get("secrets"),
    )
    codes = [e.code for e in r.of("error")]
    validate = r.last("validateResult")
    codes += [e.code for e in validate.errors] if validate is not None else []
    if bad == "truncated" and not codes:
        # The engine repaired it. That's allowed when the output is valid.
        for out in r.last("executeResult").outputs:
            fmt = _output_format(manifest, cases["negative"]["params"], out.path)
            output_check.check(job_dir / out.path, fmt)
        return
    assert codes, f"{bad}: no error and no result"
    assert set(codes) <= STRUCTURED_INPUT_ERRORS, codes


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_encrypted_input_without_password(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    if not _reads_pdf(manifest):
        pytest.skip("doesn't read PDFs")
    inputs = _replace_first(cases["cases"][0]["inputs"], "encrypted")
    job_dir, rel = _job(tmp_path, corpus_dir, inputs)
    r = run_job(
        _adapter(adapter_id),
        job_dir,
        rel,
        cases.get("encrypted", {}).get("params", cases["negative"]["params"]),
        cases["negative"].get("secrets"),
    )
    validate = r.last("validateResult")
    assert validate is not None
    assert "E_PASSWORD_REQUIRED" in [e.code for e in validate.errors]


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_secret_never_leaves_the_worker(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    if not _reads_pdf(manifest):
        pytest.skip("doesn't read PDFs")
    sentinel = corpus.SENTINEL_PASSWORD.encode("utf-8")
    case = cases["cases"][0]
    inputs = _replace_first(case["inputs"], "encrypted")
    job_dir, rel = _job(tmp_path, corpus_dir, inputs)
    secrets = {**case.get("secrets", {}), "password": corpus.SENTINEL_PASSWORD}
    params = cases.get("encrypted", {}).get("params", case["params"])
    r = run_job(_adapter(adapter_id), job_dir, rel, params, secrets)
    assert r.last("executeResult") is not None or r.of("error") or r.last("validateResult")
    assert sentinel not in b"".join(r.lines)
    assert sentinel not in r.stderr
    for path in job_dir.rglob("*"):
        if path.is_file():
            assert sentinel not in path.read_bytes(), path.relative_to(job_dir)


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_cancel_within_2_seconds(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    spec = cases["cancel"]
    params = dict(spec["params"])
    if "repeat" in spec:
        repeat = spec["repeat"]
        params[repeat["param"]] = [repeat["item"]] * int(repeat["times"])
    _skip_without_engines(spec["inputs"], manifest)
    job_dir, rel = _job(tmp_path, corpus_dir, spec["inputs"])
    w = WorkerProcess(_adapter(adapter_id), job_dir)
    try:
        w.wait_for("hello", timeout=10)
        extra = {"secrets": spec["secrets"]} if spec.get("secrets") else {}
        w.send(
            {
                "type": "execute",
                "inputs": rel,
                "outDir": "out",
                "workDir": "work",
                "params": params,
                **extra,
            }
        )
        first = w.wait_for("progress", "executeResult", "error", timeout=60)
        if first.type != "progress":
            pytest.skip("finished before it could be cancelled; make the cancel case longer")
        started = time.monotonic()
        w.send({"type": "cancel"})
        end = w.wait_for("error", "executeResult", timeout=10)
        elapsed = time.monotonic() - started
        if end.type == "executeResult":
            pytest.skip("finished before cancel arrived; make the cancel case longer")
        assert end.code == "E_CANCELLED"
        assert elapsed < CANCEL_LIMIT_S, f"{elapsed:.2f}s"
    finally:
        w.finish()


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_repeat_runs_are_equivalent(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    case = cases["cases"][0]
    _skip_without_engines(case["inputs"], manifest)
    password = case.get("expect", {}).get("password", "")
    runs = []
    for n in range(2):
        job_dir, rel = _job(tmp_path / str(n), corpus_dir, case["inputs"])
        r = run_job(_adapter(adapter_id), job_dir, rel, case["params"], case.get("secrets"))
        result = r.last("executeResult")
        assert result is not None
        runs.append(
            [
                (
                    o.suggestedName,
                    output_check.fingerprint(
                        job_dir / o.path, _output_format(manifest, case["params"], o.path), password
                    ),
                )
                for o in result.outputs
            ]
        )
    assert runs[0] == runs[1]


def test_kit_covers_every_shipping_adapter() -> None:
    assert set(IDS) >= {
        "image-convert",
        "images-to-pdf",
        "pdf-to-image",
        "pdf-merge",
        "pdf-split",
        "pdf-organize",
        "pdf-protect",
        "pdf-watermark",
        "pdf-redact",
        "pdf-compress",
        "pdf-page-numbers",
        "image-edit",
        "image-compress",
        "images-to-gif",
        "audio-join",
        "audio-edit",
        "video-join",
        "video-edit",
        "video-frames",
        "data-combine",
        "data-split",
    }
    assert sys.version_info >= (3, 12)
