# converter-jobstore

Job store, job directories, staging, garbage collection and saving ([spec section 10](../../docs/specs/phase-1.md#10-scheduler-job-store-and-garbage-collection)).

## What it does

| Area | Behavior |
|---|---|
| Job state | In memory only. Every change is checked against the section 10.1 state machine and gets the next app-wide `seq`. `snapshot()` serves `get_snapshot` and holds no paths |
| Data root | `%LOCALAPPDATA%\<id>`, `~/Library/Application Support/<id>` or `$XDG_CACHE_HOME/<id>`, where `<id>` is the bundle identifier. Refused if it's inside a sync folder |
| Job directory | `jobs/<jobId>/` with `.lease`, `in/`, `work/`, `out/`, private to the user (`0700` on Unix; on Windows the per-user `%LOCALAPPDATA%` ACL, with the sandbox grant added in T09) |
| Leases | PID plus process start time, so a reused PID isn't mistaken for the owner |
| Staging | Free space must be at least `sum(input size) × (1 + diskFactor)`. Inputs are cloned where the filesystem supports it, otherwise copied, to `in/<n>.<ext>` and made read-only. The original path stays in memory only |
| Cleanup | Success: delete `work/`. Failure or cancel: delete `in/`, `work/`, `out/`. Save or discard: delete the job. Exit: delete every job this process owns. Start: delete jobs whose owner is gone. Blocked deletes retry 5 times with backoff, then wait for the next start |
| Outputs | Must be regular files inside `out/`, reached without links or reparse points, with the claimed size |
| Saving | Copy to a temp file in the destination folder, flush, rename. A file destination replaces (the dialog confirmed it); a folder destination never replaces and adds ` (1)`, ` (2)`, … Saving over one of the job's inputs needs explicit confirmation. A sync-folder destination is reported as a notice. Worker-suggested names are sanitized |

Not here: the native dialogs (the app shell calls them and passes the chosen path), the sandbox's directory grants (T09, T10), and hash and sniff checks on outputs (the supervisor, T12).

## Known limits

- **No secure erasure.** Deleted job files may remain recoverable on disk, especially on SSDs. Don't describe cleanup as secure deletion.
- **Sync folder detection is best effort.** It covers the documented locations of OneDrive, Dropbox, iCloud Drive and Google Drive (see `src/sync.rs`). It misses Google Drive's virtual drive letter on Windows, iCloud's "Desktop and Documents" option on macOS, folders moved after setup, and other providers.
