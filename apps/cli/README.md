# lfcs

The converter from a terminal. `lfcs` reuses the app's core, so every file goes
through the same router, per-job sandbox and output checks as the desktop app.
Nothing leaves your computer and nothing is overwritten.

```sh
cargo build -p lfcs            # target/debug/lfcs
```

`lfcs` finds its adapters and Python runtime from the repository around the
binary (after `uv sync`), or from `--resources DIR` (a checkout, or an app
bundle's resources folder).

## Commands

```sh
lfcs doctor                                  # sandbox self-test, engines, adapters
lfcs inspect report.pdf photo.heic           # format, size, pages, findings
lfcs targets report.pdf                      # conversions and tools, and why one is unavailable

lfcs convert photo.png --to jpg              # saves photo.jpg here
lfcs convert scans/ --to pdf --out out/ --keep-folders
lfcs convert clip.mp4 --to png --set mode=interval --set every=10
lfcs convert a.png --to jpg --params '{"quality": 70}'

lfcs run pdf-merge a.pdf b.pdf --out merged.pdf
lfcs run pdf-split big.pdf --out pieces/
lfcs run pdf-merge a.pdf b.pdf --json        # machine-readable result
```

Folders are walked recursively (dot files and symbolic links skipped);
unrecognised files inside them are skipped with a note.

## Options

| Option | Meaning |
| --- | --- |
| `--to FORMAT` | Target format for `convert` |
| `-o, --out PATH` | Where to save (default: the current folder). A folder; for `run` with a single result it may be a file name |
| `--set KEY=VALUE` | A parameter. VALUE is JSON when it parses (`80`, `true`, `[1,2]`), else text. Repeatable; overrides `--params` |
| `--params JSON` | Parameters as a JSON object |
| `--keep-folders` | `convert`: save into the same folder layout as any folder you gave |
| `--force` | Replace an existing single-file `--out` |
| `--json` | One JSON document on stdout |
| `-q, --quiet` | No progress lines on stderr |
| `--resources DIR` | Folder with `adapters/`, `engines/` and the Python runtime |
| `--data-dir DIR` | Working folder for jobs (default: the app's data folder) |

Parameters use each adapter's `params.schema.json`; defaults from the schema are
filled in, as the app's forms do. Results go to stdout (one saved path per
line); progress and warnings go to stderr.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Done |
| 1 | A job failed (its error code and warnings are printed), or only partly succeeded |
| 2 | Usage error |
| 3 | Sandbox unavailable: the self-test failed, so nothing was processed |

## How it runs workers

The binary is its own sandbox runner: it re-executes itself as
`lfcs --sandbox-run ...` inside each job's sandbox, like the desktop app.
