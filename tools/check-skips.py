#!/usr/bin/env python3
"""Fail when pytest skipped more tests than the recorded ceiling (TST-1).

    uv run pytest --junitxml=junit.xml
    python3 tools/check-skips.py junit.xml --max 352

Under CI a missing FFmpeg or LibreOffice fails its tests instead of skipping
them (`CI=true`), but LibreOffice is only enforced on Linux, and a test can
also skip for another reason. Without a ceiling a broken engine install
could turn hundreds of tests into silent skips. This prints the skips by
reason, so a log shows what is being skipped, and exits 1 over the ceiling.

The ceiling is what a run skipped when it was set: skips that depend on the
adapter (no truncated sample for a format, adapters that don't read PDFs).
Lower it when tests stop skipping; raise it only with a reason in the commit.
Standard library only.
"""

from __future__ import annotations

import argparse
import sys
import xml.etree.ElementTree as ET  # noqa: S405 - our own pytest output
from collections import Counter


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("junit", help="pytest --junitxml output")
    ap.add_argument("--max", type=int, required=True, help="most skipped tests allowed")
    args = ap.parse_args()

    root = ET.parse(args.junit).getroot()  # noqa: S314 - our own pytest output
    reasons: Counter[str] = Counter()
    total = 0
    for case in root.iter("testcase"):
        total += 1
        skipped = case.find("skipped")
        # pytest writes an expected failure as <skipped type="pytest.xfail">.
        if skipped is not None and skipped.get("type") != "pytest.xfail":
            reasons[(skipped.get("message") or "no reason given").strip()] += 1

    skips = sum(reasons.values())
    print(f"{total} tests, {skips} skipped (ceiling {args.max})")
    for reason, n in reasons.most_common():
        print(f"  {n:4d}  {reason}")
    if total == 0:
        print("no tests found in the report", file=sys.stderr)
        return 1
    if skips > args.max:
        print(
            f"FAIL: {skips} skipped is more than the {args.max} allowed. "
            "An engine (FFmpeg, LibreOffice) is probably missing from this runner.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
