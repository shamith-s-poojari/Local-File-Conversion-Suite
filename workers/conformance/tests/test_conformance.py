"""Spec section 8.4: every shipping adapter passes the same checks.

Cases come from ``adapters/<id>/tests/conformance.yaml``; see this kit's README.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path
from typing import Any

import pikepdf
import pytest
import yaml
from converter_conformance import corpus
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


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_has_cases_readme_and_matching_class(adapter_id, manifest, cases) -> None:
    folder = ADAPTERS / adapter_id
    assert cases.get("cases"), "tests/conformance.yaml needs at least one case"
    assert {"negative", "cancel"} <= cases.keys()
    assert (folder / "README.md").is_file()
    cls = load_adapter_class(folder)
    assert cls.adapter_id == manifest["id"] == folder.name
    assert cls.fidelity == manifest["fidelity"]


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_cases_produce_valid_contained_outputs(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    for n, case in enumerate(cases["cases"]):
        job_dir, inputs = _job(tmp_path / str(n), corpus_dir, case["inputs"])
        before = _files(job_dir)
        r = run_job(_adapter(adapter_id), job_dir, inputs, case["params"], case.get("secrets"))
        result = r.last("executeResult")
        seen = [(m.type, getattr(m, "code", None)) for m in r.messages]
        assert result is not None, f"{case['name']}: {seen}"

        # Containment: everything new is under out/ or work/.
        new = _files(job_dir) - before
        assert all(p.startswith(("out/", "work/")) for p in new), sorted(new)

        # Validity and page counts.
        expect = case.get("expect", {})
        password = expect.get("password", "")
        pages = []
        for out in result.outputs:
            with pikepdf.open(job_dir / out.path, password=password) as pdf:
                assert pdf.check_pdf_syntax() == [], f"{case['name']}: {out.path} fails check"
                pages.append(len(pdf.pages))
                assert out.pages == len(pdf.pages)
        if "pages" in expect:
            assert pages == expect["pages"], case["name"]

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
    inputs = _replace_first(cases["cases"][0]["inputs"], bad)
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
            with pikepdf.open(job_dir / out.path) as pdf:
                assert pdf.check_pdf_syntax() == []
        return
    assert codes, f"{bad}: no error and no result"
    assert set(codes) <= STRUCTURED_INPUT_ERRORS, codes


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_encrypted_input_without_password(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    inputs = _replace_first(cases["cases"][0]["inputs"], "encrypted")
    job_dir, rel = _job(tmp_path, corpus_dir, inputs)
    r = run_job(
        _adapter(adapter_id),
        job_dir,
        rel,
        cases["negative"]["params"],
        cases["negative"].get("secrets"),
    )
    validate = r.last("validateResult")
    assert validate is not None
    assert "E_PASSWORD_REQUIRED" in [e.code for e in validate.errors]


@pytest.mark.parametrize(("adapter_id", "manifest", "cases"), ADAPTER_LIST, ids=IDS)
def test_secret_never_leaves_the_worker(
    adapter_id, manifest, cases, tmp_path: Path, corpus_dir: Path
) -> None:
    sentinel = corpus.SENTINEL_PASSWORD.encode("utf-8")
    case = cases["cases"][0]
    inputs = _replace_first(case["inputs"], "encrypted")
    job_dir, rel = _job(tmp_path, corpus_dir, inputs)
    secrets = {**case.get("secrets", {}), "password": corpus.SENTINEL_PASSWORD}
    r = run_job(_adapter(adapter_id), job_dir, rel, case["params"], secrets)
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
    password = case.get("expect", {}).get("password", "")
    runs = []
    for n in range(2):
        job_dir, rel = _job(tmp_path / str(n), corpus_dir, case["inputs"])
        r = run_job(_adapter(adapter_id), job_dir, rel, case["params"], case.get("secrets"))
        result = r.last("executeResult")
        assert result is not None
        runs.append(
            [
                (o.suggestedName, corpus.page_texts(job_dir / o.path, password))
                for o in result.outputs
            ]
        )
    assert runs[0] == runs[1]


def test_kit_covers_every_shipping_adapter() -> None:
    assert set(IDS) >= {
        "pdf-merge",
        "pdf-split",
        "pdf-organize",
        "pdf-protect",
        "pdf-watermark",
    }
    assert sys.version_info >= (3, 12)
