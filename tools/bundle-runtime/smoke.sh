#!/usr/bin/env bash
# Packaged smoke test (REL-2): run the command line against a bundled
# resources folder, the way an installed app finds its runtime.
#
#   tools/bundle-runtime/smoke.sh PATH/TO/lfcs RESOURCES_DIR
#
# RESOURCES_DIR holds python/, adapters/ and engines/: the bundle-runtime
# output, or the resources folder inside a built .app, or one unpacked from a
# .deb or an installer. Development mode is switched off, so the checkout's
# .venv can't be what runs.
#
# 1. `lfcs doctor`: opens the app on that runtime, runs the sandbox
#    self-test (exit 3 if it fails) and lists engines and adapters.
# 2. Converts a Markdown file to HTML: a worker started from the bundled
#    Python, inside the sandbox, with the bundled adapters and packages.
#
# FFmpeg and LibreOffice are not needed and not tested here.

set -euo pipefail

lfcs="${1:?usage: smoke.sh PATH/TO/lfcs RESOURCES_DIR}"
res="${2:?usage: smoke.sh PATH/TO/lfcs RESOURCES_DIR}"
unset LFCS_DEV

# Git Bash on Windows: hand the program native paths.
native() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi; }

if [[ ! -f "$res/python/bin/python3" && ! -f "$res/python/python.exe" ]]; then
  echo "smoke: no bundled Python under $res/python" >&2
  exit 1
fi
for need in adapters/pdf-merge/manifest.yaml engines/engines.yaml; do
  [[ -f "$res/$need" ]] || { echo "smoke: $res/$need is missing" >&2; exit 1; }
done
if [[ -e "$res/adapters/dev-fake" ]]; then
  echo "smoke: the development-only adapter is in the bundle" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
resources="$(native "$res")"

echo "== lfcs doctor"
"$lfcs" doctor --resources "$resources" | tee "$work/doctor.txt"
grep -q '^sandbox: passed' "$work/doctor.txt"
if grep -q 'dev-fake' "$work/doctor.txt"; then
  echo "smoke: doctor lists the development-only adapter" >&2
  exit 1
fi

echo "== convert Markdown to HTML with the bundled runtime"
printf '# Smoke test\n\nA short note.\n' > "$work/note.md"
"$lfcs" convert "$(native "$work/note.md")" --to html --out "$(native "$work/out")" \
  --resources "$resources"
grep -q 'Smoke test' "$work"/out/note*.html
echo "smoke: ok"
