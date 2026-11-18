#!/usr/bin/env python3
"""Build the worker runtime a release app carries (REL-1).

    python3 tools/bundle-runtime/bundle_runtime.py [--out DIR] [--python-version X.Y.Z]

Writes, by default into `target/bundle-runtime/`, the layout that
`crates/app-core/src/config.rs` expects in an app's resources folder:

    python/               a relocatable CPython (python-build-standalone, by uv)
      bin/python3         (Windows: python/python.exe)
      lib/.../site-packages   the locked third-party packages and converter_sdk
    adapters/             every adapter, without tests, caches and dev-only ones
    engines/engines.yaml  the approved-engine catalog
    tauri.resources.conf.json   a Tauri config that adds the three folders as
                          bundle resources: `tauri build --config <this file>`
    BUNDLE-MANIFEST.txt   what went in, for the release notes

The Python is pinned (`--python-version`, default below). The packages come
from `uv.lock` (`uv export --frozen`, with hashes, wheels only), so a build
installs exactly what the lock names. FFmpeg and LibreOffice are not
included: they stay user-installed (docs/engines.md).

Needs `uv` on PATH and network access (it downloads the interpreter and the
wheels). Standard library only. Not run by `pnpm check`; the release workflow
(.github/workflows/release.yml) runs it, then a packaged smoke test.

Why not just list the folders in tauri.conf.json: tauri-build stops with an
error when a resource folder doesn't exist, which would break every
`cargo build` and `cargo clippy` of the desktop crate on a checkout that has
not run this script. The generated config is only passed to release builds.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

# The interpreter the release ships. 3.13 is the line CI tests on
# (.python-version); the lock allows >=3.12,<3.14. Change it here, and the
# release notes say which CPython a build carries (BUNDLE-MANIFEST.txt).
DEFAULT_PYTHON = "3.13.15"

# Packages in the dev dependency group that a release must not carry. Today
# the workers' libraries (pikepdf, pymupdf, pillow, ...) are listed in that
# group of the root pyproject.toml, next to the tools, so the tools are
# pruned by name. If the libraries move to their own group, use it instead
# (`--only-group`) and delete this list.
TOOLS_TO_PRUNE = ["pytest", "ruff", "datamodel-code-generator"]

# Imported with `-I` after the install, to catch a broken bundle here and not
# on a user's machine. (Distribution name -> module.)
MUST_IMPORT = [
    "converter_sdk",
    "pydantic",
    "pikepdf",
    "pymupdf",
    "PIL",
    "pillow_heif",
    "pypdfium2",
    "reportlab",
    "openpyxl",
    "yaml",
    "lxml",
    "markdown_it",
    "pdf2docx",
]

REPO = Path(__file__).resolve().parents[2]
MARKER = ".bundle-runtime"


def run(cmd: list[str | Path], **kw) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run(  # noqa: S603 - fixed commands built above
        [str(c) for c in cmd], check=True, text=True, **kw
    )


def interpreter(root: Path) -> Path:
    return root / ("python.exe" if sys.platform == "win32" else "bin/python3")


def clear(out: Path) -> None:
    """Empty `out`, but only a folder this script made (or an empty one)."""
    if not out.exists():
        return
    if any(out.iterdir()) and not (out / MARKER).exists():
        sys.exit(f"{out} exists and was not made by this script; choose another --out")
    shutil.rmtree(out)


def install_python(version: str, out: Path, work: Path) -> Path:
    pbs = work / "pbs"
    run(
        ["uv", "python", "install", version, "--install-dir", pbs, "--no-bin", "--no-registry"],
    )
    # uv names the folder cpython-<version>-<os>-<arch>-<libc>; a minor-version
    # alias (cpython-3.13-...) may sit beside it, and is not this.
    real = [d for d in pbs.glob(f"cpython-{version}-*") if d.is_dir() and not d.is_symlink()]
    if len(real) != 1:
        sys.exit(f"expected one cpython-{version}-* folder in {pbs}, found {real}")
    target = out / "python"
    shutil.move(str(real[0]), str(target))
    if not interpreter(target).is_file():
        sys.exit(f"no interpreter at {interpreter(target)}")
    return target


def purelib(python: Path) -> Path:
    code = "import sysconfig; print(sysconfig.get_paths()['purelib'])"
    text = subprocess.run(  # noqa: S603
        [str(python), "-I", "-c", code], check=True, capture_output=True, text=True
    ).stdout.strip()
    return Path(text)


def export_requirements(work: Path) -> Path:
    req = work / "requirements.txt"
    cmd: list[str | Path] = [
        "uv", "export", "--frozen", "--all-groups", "--no-emit-workspace",
        "--format", "requirements-txt", "--no-header", "--output-file", req,
    ]  # fmt: skip
    for tool in TOOLS_TO_PRUNE:
        cmd += ["--prune", tool]
    # uv prints the requirements as well as writing them; the log needn't.
    run(cmd, cwd=REPO, stdout=subprocess.DEVNULL)
    names = {
        m.group(1).lower()
        for line in req.read_text(encoding="utf-8").splitlines()
        if (m := re.match(r"^([A-Za-z0-9._-]+)==", line))
    }
    for needed in ("pikepdf", "pymupdf", "pillow", "pydantic", "pyyaml"):
        if needed not in names:
            sys.exit(f"the exported requirements lack {needed}: check TOOLS_TO_PRUNE")
    for tool in TOOLS_TO_PRUNE:
        if tool in names:
            sys.exit(f"{tool} is still in the exported requirements")
    return req


def install_packages(python: Path, site: Path, req: Path) -> None:
    site.mkdir(parents=True, exist_ok=True)
    common = ["--python", python, "--target", site, "--no-deps", "--link-mode", "copy"]
    # Hashes from the lock are checked, and only wheels are installed: an
    # sdist would run build code on the release machine.
    run(["uv", "pip", "install", *common, "--only-binary", ":all:", "--require-hashes",
         "-r", req], cwd=REPO)  # fmt: skip
    # The workspace's own packages, as real (non-editable) installs.
    run(["uv", "pip", "install", *common, REPO / "workers" / "sdk"], cwd=REPO)


def precompile(python: Path, site: Path) -> None:
    """Ship bytecode: a sandboxed worker can't write __pycache__, and the
    resources folder may be read-only. `unchecked-hash` keeps it valid when a
    packager changes file times."""
    run([python, "-I", "-m", "compileall", "-q", "--invalidation-mode", "unchecked-hash", site])


def copy_adapters(out: Path) -> list[str]:
    src = REPO / "adapters"
    dst = out / "adapters"
    kept: list[str] = []

    def dev_only(d: Path) -> bool:
        manifest = d / "manifest.yaml"
        text = manifest.read_text(encoding="utf-8") if manifest.is_file() else ""
        return re.search(r"^devOnly:\s*true\s*$", text, re.M) is not None

    ignore = shutil.ignore_patterns("tests", "__pycache__", "*.pyc", ".pytest_cache")
    dst.mkdir()
    for item in sorted(src.iterdir()):
        if item.is_dir():
            if dev_only(item):
                print(f"skipping dev-only adapter {item.name}")
                continue
            shutil.copytree(item, dst / item.name, ignore=ignore)
            kept.append(item.name)
        else:
            shutil.copy2(item, dst / item.name)
    return kept


def verify(python: Path) -> None:
    code = (
        "import importlib\n"
        f"for m in {MUST_IMPORT!r}:\n"
        "    importlib.import_module(m)\n"
        "print('imports ok')"
    )
    run([python, "-I", "-c", code])
    # Nothing may point back into the checkout (an editable install would).
    site = purelib(python)
    for pth in site.glob("*.pth"):
        text = pth.read_text(encoding="utf-8", errors="replace")
        if str(REPO) in text:
            sys.exit(f"{pth} points into the checkout: the bundle isn't relocatable")
    for direct in site.glob("*.dist-info/direct_url.json"):
        if '"editable": true' in direct.read_text(encoding="utf-8"):
            sys.exit(f"{direct.parent.name} is an editable install")


def write_resources_config(out: Path) -> Path:
    cfg = out / "tauri.resources.conf.json"
    resources = {(out / name).as_posix(): name for name in ("python", "adapters", "engines")}
    cfg.write_text(json.dumps({"bundle": {"resources": resources}}, indent=2) + "\n")
    return cfg


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", type=Path, default=REPO / "target" / "bundle-runtime")
    ap.add_argument("--python-version", default=DEFAULT_PYTHON, help="exact CPython, X.Y.Z")
    args = ap.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.python_version):
        sys.exit("--python-version must be an exact version such as 3.13.15")
    if shutil.which("uv") is None:
        sys.exit("uv is not on PATH")

    out = args.out.resolve()
    clear(out)
    out.mkdir(parents=True)
    (out / MARKER).write_text("made by tools/bundle-runtime/bundle_runtime.py\n")
    work = out / ".work"
    work.mkdir()

    python_root = install_python(args.python_version, out, work)
    python = interpreter(python_root)
    site = purelib(python)
    if python_root not in site.parents:
        sys.exit(f"site-packages {site} is not inside the bundled Python")

    req = export_requirements(work)
    install_packages(python, site, req)
    precompile(python, site)
    adapters = copy_adapters(out)
    (out / "engines").mkdir()
    shutil.copy2(REPO / "engines" / "engines.yaml", out / "engines" / "engines.yaml")
    verify(python)

    version = subprocess.run(  # noqa: S603
        [str(python), "-I", "-c", "import sys; print(sys.version)"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()  # fmt: skip
    packages = sorted(
        line.split(" ")[0].strip()
        for line in req.read_text(encoding="utf-8").splitlines()
        if "==" in line and not line.startswith(" ")
    )
    (out / "BUNDLE-MANIFEST.txt").write_text(
        f"Python: {version}\n\nPackages (from uv.lock):\n"
        + "".join(f"  {p}\n" for p in packages)
        + f"\nAdapters ({len(adapters)}):\n"
        + "".join(f"  {a}\n" for a in adapters),
        encoding="utf-8",
    )
    shutil.rmtree(work)
    cfg = write_resources_config(out)
    size = sum(f.stat().st_size for f in out.rglob("*") if f.is_file()) / 1e6
    print(f"\nruntime ready in {out} ({size:.0f} MB)\nTauri config: {cfg}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
