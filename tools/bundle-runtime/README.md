# bundle-runtime

Builds the worker runtime a release app carries (REL-1): a relocatable Python with the locked packages, the adapters and the engine catalog.

**Status:** written and partly checked; **not run end to end yet**. The pieces that don't download were run (the adapter copy, the requirements export, the generated config, and `smoke.sh` against a hand-made folder of the same layout). The interpreter download, the `uv pip install` of the whole lock, and the Tauri packaging have not been run on any system. The release workflow (`.github/workflows/release.yml`) is the first place they will run.

## Use

```bash
python3 tools/bundle-runtime/bundle_runtime.py            # into target/bundle-runtime
python3 tools/bundle-runtime/bundle_runtime.py --out DIR --python-version 3.13.15
```

Needs `uv` on `PATH` and network access (the interpreter and the wheels are downloaded). The script is Python's standard library only, so it runs the same on macOS, Linux and Windows. It refuses to empty a folder it didn't make.

## What it writes

```
target/bundle-runtime/
  python/                 pinned CPython (python-build-standalone, installed by uv)
    bin/python3           (Windows: python/python.exe)
    lib/python3.13/site-packages/   locked packages + converter_sdk (Windows: Lib/site-packages)
  adapters/               every adapter except dev-only ones; no tests, no caches
  engines/engines.yaml    the approved-engine catalog
  tauri.resources.conf.json   Tauri config: the three folders as bundle resources
  BUNDLE-MANIFEST.txt     the Python version, every package and adapter
```

This is the layout `crates/app-core/src/config.rs` expects under an app's resources folder (`python/bin/python3` or `python/python.exe`, `adapters/`, `engines/engines.yaml`). A release app is its own sandbox runner (`<app> --sandbox-run`), so no extra binary is shipped.

## How the runtime is made

1. `uv python install <exact version> --install-dir … --no-bin --no-registry`, then the folder is moved to `python/`. The version is pinned in the script (`DEFAULT_PYTHON`).
2. `uv export --frozen --all-groups --no-emit-workspace --prune pytest --prune ruff --prune datamodel-code-generator` writes the locked requirements with hashes. The worker libraries (pikepdf, PyMuPDF, Pillow, and the rest) are in the root `pyproject.toml`'s `dev` group today, next to the tools, so the tools are pruned by name. A dependency group just for the workers would replace this list.
3. `uv pip install --target <site-packages> --no-deps --only-binary :all: --require-hashes`: only wheels, with the lock's hashes checked, and copies rather than links.
4. `converter_sdk` is installed from `workers/sdk` as a normal (not editable) package.
5. The packages are byte-compiled with `unchecked-hash` pycs, because a sandboxed worker can't write `__pycache__` and a packager may change file times.
6. The script imports every engine module with `python -I`, and refuses a bundle that has an editable install or a `.pth` pointing at the checkout.

## Using it in a build

`tauri.conf.json` doesn't list these folders. `tauri-build` stops with an error when a resource folder is missing, which would break every `cargo build` and `cargo clippy` of the desktop crate on a checkout that hasn't run this script. The generated config is passed to release builds only:

```bash
pnpm --filter @converter/desktop tauri build \
  --config "$PWD/target/bundle-runtime/tauri.resources.conf.json" --bundles dmg   # or nsis, deb
```

## Smoke test

```bash
tools/bundle-runtime/smoke.sh target/release/lfcs target/bundle-runtime
```

Runs `lfcs doctor --resources …` (sandbox self-test; fails if it doesn't pass) and converts a Markdown file to HTML through a worker started from the bundled Python. Development mode is switched off, so the checkout's `.venv` can't be what runs. The release workflow runs it twice: on this folder, and on the resources inside the finished `.app`, `.deb` or installer.

## Not covered

- **FFmpeg and LibreOffice** aren't bundled ([engines](../../docs/engines.md)).
- **Signing.** The bundle is unsigned. On macOS every nested binary in `python/` (interpreter, libraries, extension modules) will need signing for notarization, and where nested code may sit in the bundle is open in spike S2.
- **Size.** It hasn't been measured. OpenCV (a `pdf2docx` dependency) and NumPy are the large wheels.
- **Other CPUs.** Each system builds for itself; there is no cross-building, and no Linux AArch64 or Windows ARM job.
