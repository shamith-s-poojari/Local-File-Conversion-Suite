# lfcs

The converter from a terminal. `lfcs` reuses the app's core, so every file goes
through the same router, per-job sandbox and output checks as the desktop app.
Nothing leaves your computer and nothing is overwritten.

```sh
cargo build -p lfcs            # target/debug/lfcs
```

`lfcs` finds its adapters and Python runtime in an app bundle's resources
folder (`--resources DIR`). A repository checkout is only used in development
mode: pass `--dev` (or set `LFCS_DEV=1`) after `uv sync`. Without it, `lfcs`
never looks in the current folder or beside the binary for a runtime, so a
checkout you happen to be standing in can't run code as you.

```sh
cargo run -p lfcs -- --dev doctor
LFCS_DEV=1 target/debug/lfcs convert photo.png --to jpg
```

## Commands

```sh
lfcs doctor                                  # sandbox self-test, engines, adapters
lfcs inspect report.pdf photo.heic           # format, size, pages, findings
lfcs targets report.pdf                      # conversions and tools, and why one is unavailable

lfcs convert photo.png --to jpg              # saves photo.jpg here
lfcs convert scans/ --to pdf --out out/ --keep-folders
lfcs convert mixed/ --to jpg --skip-same     # files that are already JPEG are left out
lfcs convert clip.mp4 --to png --set mode=interval --set every=10
lfcs convert a.png --to jpg --params '{"quality": 70}'

lfcs run pdf-merge a.pdf b.pdf --out merged.pdf
lfcs run pdf-split big.pdf --out pieces/
lfcs run pdf-compress scans/ --out small/ --keep-folders
lfcs run pdf-merge a.pdf b.pdf --json        # machine-readable result
```

Folders are walked by the app core, with the same rules as dropping a folder
on the app: symbolic links, hidden and system files and anything deeper than
eight levels are skipped; unrecognised files inside them are skipped with a
note. At most 200 files are taken in one run; the rest are reported on stderr
and as `folderTruncated` in `--json` (a list of `{folder, count}`).

## Passwords

A password is never an argument (it would show in `ps` and shell history), so
there is no `--password`, and a parameter such as `--set userPassword=...` is
refused. Give it on standard input or in the environment:

```sh
printf '%s\n' "$PW" | lfcs run pdf-protect report.pdf --password-stdin --out locked.pdf
LFCS_PASSWORD=... lfcs inspect locked.pdf
```

`--password-stdin` reads the first line of standard input (and wins over
`LFCS_PASSWORD`); on a terminal it asks with typing hidden (Unix; elsewhere
pipe it in). Inside `lfcs` the copies it makes are overwritten after use, as
far as the standard library allows; the environment block and the copy the
core holds are not in its reach. The password opens encrypted input files, and for
`pdf-protect` it is the password to set. One password serves every file in a
run; files that need different ones are converted separately.

## Options

| Option | Meaning |
| --- | --- |
| `--to FORMAT` | Target format for `convert` |
| `-o, --out PATH` | Where to save (default: the current folder). A folder; for `run` with a single result it may be a file name |
| `--set KEY=VALUE` | A parameter. VALUE is JSON when it parses (`80`, `true`, `[1,2]`), else text. Repeatable; overrides `--params` |
| `--params JSON` | Parameters as a JSON object |
| `--skip-same` | `convert`: leave out files already in the target format (an error if none are left; `--json` reports `skippedSameFormat`) |
| `--keep-folders` | `convert` and `run`: save into the same folder layout as any folder you gave, under `--out` (which is then a folder) |
| `--force` | Replace an existing single-file `--out` |
| `--dev` | Use the repository checkout and its `.venv` as the runtime (same as `LFCS_DEV=1`) |
| `--password-stdin` | Read a password from the first line of standard input (or use `LFCS_PASSWORD`) |
| `--json` | One JSON document on stdout |
| `-q, --quiet` | No progress lines on stderr |
| `--resources DIR` | Folder with `adapters/`, `engines/` and the Python runtime (an installed app's resources; a checkout also needs `--dev`) |
| `--data-dir DIR` | Working folder for jobs (default: the app's data folder) |

Parameters use each adapter's `params.schema.json`; defaults from the schema are
filled in, as the app's forms do. Results go to stdout (one saved path per
line, as the save reported them); progress and warnings go to stderr. Error
codes, reasons and warnings are explained in the same words as the app's
(`apps/desktop/src/text.ts`; a test keeps the two in step).

## JSON

With `--json` a failure is always one document on stdout:

```json
{"ok": false, "error": {"code": "E_INPUT_CORRUPT", "reason": "inputCorrupt", "message": "This file can't be read. ..."}}
```

`code` is an `E_*` code (the app's, or `E_USAGE`, `E_NO_ROUTE`,
`E_NO_USABLE_FILES`, `E_SAVE_FAILED`, `E_PARTIAL` and `E_CANCELLED` from the
command line), `reason` a camelCase name for the cause, `message` text for
people. A job's report adds `state`, `warnings`, `skipped` and
`folderTruncated` around it. `inspect` also puts each file's own failure in
`files[].error`; `doctor --json` has `ok` too, and the same `error` when the
sandbox self-test fails.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Done |
| 1 | A job failed (its error code and warnings are printed), or only partly succeeded |
| 2 | Usage error |
| 3 | Sandbox unavailable: the self-test failed, so nothing was processed |
| 130 | Stopped by Ctrl-C (or SIGTERM): the job was cancelled, nothing was saved and its files were removed |

Ctrl-C asks the running job to stop and waits for its folder to be removed; a
second Ctrl-C exits at once (leftover job folders are removed at the next
start).

## How it runs workers

The binary is its own sandbox runner: it re-executes itself as
`lfcs --sandbox-run ...` inside each job's sandbox, like the desktop app.
