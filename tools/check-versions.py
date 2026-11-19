#!/usr/bin/env python3
"""Fail unless every manifest that carries a version carries the same one (REL-6).

    python3 tools/check-versions.py [--tag v0.1.0]

Checked: `[workspace.package] version` in Cargo.toml, package.json,
apps/desktop/package.json, packages/protocol/package.json, every `version`
in a pyproject.toml of the repository (the workspace root, workers/sdk,
workers/conformance), and `version` in apps/desktop/src-tauri/tauri.conf.json
if it has one (without it Tauri uses the Cargo version, which is the intent).
With `--tag`, the version must also equal the tag without its `v` (the
release workflow passes the pushed tag). A crate that says
`version.workspace = true` follows Cargo.toml; one that sets its own version
is checked too. Needs Python 3.11 (tomllib).
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# crates/sniffer/fuzz is a separate cargo-fuzz crate that is never released.
SKIP_DIRS = {"node_modules", "target", ".venv", ".git", "dist", "fuzz"}


def files(pattern: str) -> list[Path]:
    return sorted(p for p in ROOT.rglob(pattern) if not SKIP_DIRS & set(p.relative_to(ROOT).parts))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--tag", help="a release tag such as v0.1.0, which must match")
    args = ap.parse_args()
    found: dict[str, str] = {}
    problems: list[str] = []

    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    found["Cargo.toml [workspace.package]"] = cargo["workspace"]["package"]["version"]

    for path in files("Cargo.toml"):
        rel = path.relative_to(ROOT)
        if rel.as_posix() == "Cargo.toml":
            continue
        pkg = tomllib.loads(path.read_text(encoding="utf-8")).get("package")
        if pkg is None:
            continue
        v = pkg.get("version")
        if isinstance(v, dict) and v.get("workspace") is True:
            continue
        if v is None:
            problems.append(f"{rel}: has no version (use `version.workspace = true`)")
        else:
            found[str(rel)] = v

    for path in files("package.json"):
        v = json.loads(path.read_text(encoding="utf-8")).get("version")
        if v is not None:
            found[str(path.relative_to(ROOT))] = v

    for path in files("pyproject.toml"):
        v = tomllib.loads(path.read_text(encoding="utf-8")).get("project", {}).get("version")
        if v is not None:
            found[str(path.relative_to(ROOT))] = v

    tauri = ROOT / "apps/desktop/src-tauri/tauri.conf.json"
    v = json.loads(tauri.read_text(encoding="utf-8")).get("version")
    if v is not None:
        found[str(tauri.relative_to(ROOT))] = v

    wanted = found["Cargo.toml [workspace.package]"]
    for where, v in found.items():
        mark = "ok  " if v == wanted else "FAIL"
        print(f"{mark} {v:10} {where}")
        if v != wanted:
            problems.append(f"{where}: {v}, but Cargo.toml says {wanted}")
    if args.tag and args.tag != f"v{wanted}":
        problems.append(f"tag {args.tag} is not v{wanted}")
    for p in problems:
        print(p, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
