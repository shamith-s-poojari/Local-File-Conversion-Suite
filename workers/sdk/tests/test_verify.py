"""Running an adapter's output verifier in a fresh process (converter_sdk.verify)."""

from __future__ import annotations

from pathlib import Path

import pytest
from converter_conformance.harness import context, make_job_dir
from converter_sdk.errors import AdapterError
from converter_sdk.verify import run_verifier


def job(tmp_path: Path):
    src = tmp_path / "a.txt"
    src.write_text("x")
    root, rel = make_job_dir(tmp_path / "job", [src])
    ctx, _ = context(root, rel)
    out = ctx.job_dir / "out" / "0.txt"
    out.write_text("y")
    return ctx, out


def script(tmp_path: Path, body: str) -> Path:
    path = tmp_path / "verify.py"
    path.write_text(body)
    return path


def test_the_report_comes_back_and_the_verifier_sees_only_the_relative_path(tmp_path: Path) -> None:
    ctx, out = job(tmp_path)
    body = (
        "import json, os, sys\n"
        "req = json.loads(sys.stdin.read())\n"
        "print(json.dumps({'findings': [], 'seen': req, 'cwd': os.getcwd(),"
        " 'isolated': bool(sys.flags.isolated)}))\n"
    )
    report = run_verifier(ctx, script(tmp_path, body), out, {"mode": "strict"})
    assert report is not None
    assert report["seen"] == {"mode": "strict", "output": "out/0.txt"}
    assert report["isolated"] is True
    assert Path(report["cwd"]).resolve() == ctx.job_dir.resolve()


@pytest.mark.parametrize(
    "body",
    [
        "import sys\nsys.exit(3)\n",
        "print('not json')\n",
        "print('[1, 2]')\n",
        "",
    ],
    ids=["crash", "garbage", "not an object", "silent"],
)
def test_a_verifier_that_cannot_report_gives_none(tmp_path: Path, body: str) -> None:
    ctx, out = job(tmp_path)
    assert run_verifier(ctx, script(tmp_path, body), out) is None


def test_a_hung_verifier_times_out(tmp_path: Path) -> None:
    ctx, out = job(tmp_path)
    with pytest.raises(AdapterError) as e:
        run_verifier(ctx, script(tmp_path, "import time\ntime.sleep(60)\n"), out, timeout_s=0.3)
    assert e.value.code == "E_TIMEOUT"


def test_a_cancelled_job_stops_the_verifier(tmp_path: Path) -> None:
    ctx, out = job(tmp_path)
    ctx._cancel.set()
    with pytest.raises(AdapterError) as e:
        run_verifier(ctx, script(tmp_path, "import time\ntime.sleep(60)\n"), out)
    assert e.value.code == "E_CANCELLED"


def test_a_noisy_stderr_does_not_block_the_verifier(tmp_path: Path) -> None:
    ctx, out = job(tmp_path)
    body = "import sys\nsys.stderr.write('x' * 5_000_000)\nprint('{\"findings\": []}')\n"
    assert run_verifier(ctx, script(tmp_path, body), out) == {"findings": []}


def test_a_report_larger_than_the_pipe_is_read_while_it_runs(tmp_path: Path) -> None:
    ctx, out = job(tmp_path)
    body = (
        "import json\n"
        "rows = [{'kind': 'x', 'detail': 'y' * 100} for _ in range(2000)]\n"
        "print(json.dumps({'findings': rows}))\n"
    )
    report = run_verifier(ctx, script(tmp_path, body), out, timeout_s=10)
    assert report is not None and len(report["findings"]) == 2000


def test_an_absurd_report_is_rejected(tmp_path: Path) -> None:
    ctx, out = job(tmp_path)
    body = "import sys\nsys.stdout.write('[' + '0,' * 3_000_000 + '0]')\n"
    assert run_verifier(ctx, script(tmp_path, body), out, timeout_s=20) is None
