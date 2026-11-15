# Privacy

**Your files are processed on your computer and are never uploaded.** This page says what that means in practice, how it is enforced and tested, and where its limits are.

## What the app does with your files

| | |
|---|---|
| **Reading** | When you choose or drop files, the app reads only what it needs to identify them: the first and last 64 KB, and for Office files the ZIP directory. Online-only files (OneDrive, iCloud Drive and similar) aren't read until you agree to download them |
| **Processing** | Each job works on a private copy of its files, in its own folder in the app's data folder (`~/Library/Application Support/io.github.shamith-s-poojari.local-file-conversion-suite` on macOS). The data folder must not be inside a sync folder; the app refuses to start processing if it is |
| **Saving** | Results are written only where you choose in the save dialog. The app warns if that folder syncs to the cloud, and asks before replacing one of your original files |
| **Afterwards** | A job's folder is deleted when you save or discard its result, and every job folder is deleted when the app quits. Folders left behind by a crash are deleted at the next start |
| **History** | None. The app keeps no list of recent files, and nothing about your files survives a restart |

**Deletion is not secure erasure.** Files are deleted normally. On SSDs especially, deleted data can sometimes be recovered with specialist tools until the space is reused. If that matters, use full-disk encryption (FileVault, BitLocker or LUKS).

## No network

- The app has **no network code**: no update checks, telemetry, crash reports, font downloads or online help. Updates are downloaded manually.
- Every processing engine runs in a **sandbox with no network access**: no internet, no connections to other programs on your computer, no name lookups. See [Sandboxing](#sandboxing).
- Links, scripts, form actions and remote references inside documents are **never followed or run**. Redaction removes them from its output.

### How it is tested

- **Canary test** (`crates/app-core/tests/network.rs`): every tool runs on documents whose links, actions, scripts, attachments and metadata all point at a listener on the test machine. The test fails if anything connects to it, or if any engine process holds a network connection. Positive controls make sure the test can see a connection when there is one.
- **Packet capture** (`tools/network-test/capture.sh`, nightly): records every packet during a full run and fails on any packet from the app's processes.

### The WebView

The app's window uses the operating system's web view (WebKit on macOS, WebView2 on Windows, WebKitGTK on Linux). The operating system may make its own background connections for that component, for example to check certificate revocation or update WebView2. **These are not yet measured.** Spike S4 will list each one by process and destination, turn off what can be turned off, and record the rest here. Until then, the packet-capture test treats any WebView traffic as a failure.

The web view only ever shows the app's own interface. It can't load web pages, open links or reach the network: its content security policy allows no remote sources, and navigation away from the app is blocked.

## Sandboxing

Each job's engines run in a sandbox that can:

- read the app's own runtime and tools;
- read and write the job's private folder;
- nothing else: not your home folder, not other jobs, not the network.

The app checks this at every start with a self-test (Settings shows the result). If the sandbox doesn't work, **processing is turned off**: the app never runs an engine without it.

| OS | Sandbox | Status |
|---|---|---|
| macOS | A separate Seatbelt profile per job | Working; self-test passes |
| Linux | Bubblewrap namespaces | Written, not yet verified on Linux |
| Windows | AppContainer | Not implemented yet; processing is off on Windows |

## What the app stores

| What | Where | How long |
|---|---|---|
| Copies of your files, intermediate files and results | The job's folder in the data folder | Until you save or discard, or the app quits |
| Page previews | The job's folder | Same |
| Passwords you type | Memory only, never on disk, in logs or in command lines | Until the file is removed from the app, or the app quits |

The app writes no logs containing file names, document text or passwords.

## Metadata in results

Results keep the document properties of their source (title, author and so on) unless the tool removes them. Redaction always removes all document properties and XMP metadata, keeping only a neutral "Producer".

The tools don't add your name, folder paths or the time to results, with one exception: **Merge's per-file bookmarks are titled with the original file names** (choose "keep" or "none" for bookmarks to avoid that).

## Finding and removing hidden details

When you add a file, its sandboxed worker lists what the file gives away (location, camera or device, author, software, dates, comments, tracked changes, hidden content, tags) and what it could do when opened (macros, scripts, embedded files, outside links). The list is made on your device. A finding names what was found, or a short value such as a camera model or coordinates; it never contains the document's text.

**Remove hidden details** (`file-clean`, `media-clean`) removes:

- photos: EXIF (including GPS and the camera's serial number), XMP, IPTC, comments and PNG text, without changing a pixel; a rotated JPEG, TIFF and AVIF are re-encoded once;
- PDF: document properties, XMP, thumbnails and private application data, and by default comments; hidden layers on request;
- Word, Excel and PowerPoint: author, last editor, dates, company, template, custom properties and the thumbnail, and by default comments;
- audio and video: every container, stream and chapter tag, and cover art on request, without re-encoding.

It doesn't remove what is in the content itself: a face or a street sign in a photo, text inside a picture, or tracked changes (they are reported; accept or reject them in your editor first). Legacy `.doc`, `.xls` and `.ppt` files aren't accepted. Each result is read again, and the job fails if a location, device, author or date is still there.

**Make safe to open** (`file-disarm`) removes macros, ActiveX, embedded files and objects, scripts and automatic actions, form submissions, and links that load something from outside the file. Plain web links stay unless you ask. An independent check in a fresh process must find nothing active, or no file is produced.

## Clipboard

Paste turns a picture, a table or text into a file in the app's data folder; it is deleted when you remove it, clear the selection or quit. Copy puts a result on the clipboard from the app's core: text as it is, and a picture after its own sandboxed worker has decoded it to PNG, so the core never parses the picture itself. The interface has no clipboard permission.

## Known gaps

- WebView traffic is not yet measured (spike S4).
- The Windows sandbox isn't implemented, so processing is off on Windows.
- Sync-folder detection covers OneDrive, Dropbox, iCloud Drive and Google Drive at their standard locations, and is **best effort**: a sync client installed elsewhere may not be recognised.
