//! Which engines this installation has (ADR 0003).
//!
//! Python engines are importable modules of the worker interpreter. External
//! engines (FFmpeg, LibreOffice) are binaries found in the usual install
//! locations; nothing is downloaded. The result decides which conversions are
//! offered (`Availability`) and which sandbox grants each adapter gets.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use converter_router::Registry;
use converter_sandbox::Grant;

/// Engine name (as in `engines/engines.yaml`) → Python module to import.
const PYTHON_ENGINES: &[(&str, &str)] = &[
    ("pikepdf", "pikepdf"),
    ("qpdf", "pikepdf"),
    ("pymupdf", "pymupdf"),
    ("pypdfium2", "pypdfium2"),
    ("reportlab", "reportlab"),
    ("pillow", "PIL"),
    ("pillow-heif", "pillow_heif"),
    ("pdf2docx", "pdf2docx"),
    ("openpyxl", "openpyxl"),
    ("python-pptx", "pptx"),
    ("markdown-it-py", "markdown_it"),
    ("lxml", "lxml"),
    ("pyyaml", "yaml"),
    ("pdfplumber", "pdfplumber"),
    ("img2pdf", "img2pdf"),
];

/// Where binaries are looked for, after `PATH`. Apps started from the
/// Finder or a desktop launcher get a minimal `PATH`, so the usual install
/// folders are listed explicitly.
#[cfg(not(windows))]
fn bin_dirs(_name: &str) -> Vec<PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
        .iter()
        .map(PathBuf::from)
        .collect()
}

/// Windows: WinGet's links, Scoop and Chocolatey package folders (their
/// shims would start the real binary as a second process, from a folder the
/// sandbox doesn't grant), and the folders people unzip FFmpeg into.
#[cfg(windows)]
fn bin_dirs(name: &str) -> Vec<PathBuf> {
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(local) = var("LOCALAPPDATA") {
        dirs.push(local.join(r"Microsoft\WinGet\Links"));
    }
    let package = if name.starts_with("ff") {
        "ffmpeg"
    } else {
        name
    };
    let scoop = var("SCOOP").or_else(|| var("USERPROFILE").map(|h| h.join("scoop")));
    if let Some(scoop) = scoop {
        dirs.push(scoop.join("apps").join(package).join(r"current\bin"));
        dirs.push(scoop.join("apps").join(package).join("current"));
    }
    if let Some(data) = var("ProgramData") {
        let lib = data.join(r"chocolatey\lib");
        dirs.push(lib.join(package).join("tools").join(package).join("bin"));
        dirs.push(
            lib.join(format!("{package}-full"))
                .join("tools")
                .join(package)
                .join("bin"),
        );
    }
    for root in [var("ProgramFiles"), Some(PathBuf::from(r"C:\"))]
        .into_iter()
        .flatten()
    {
        dirs.push(root.join(package).join("bin"));
    }
    dirs
}

/// LibreOffice's own binary, run directly: on Linux the `soffice` script
/// needs a shell and starts `oosplash`, neither of which the sandbox has.
fn soffice_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        out.push("/Applications/LibreOffice.app/Contents/MacOS/soffice".into());
        if let Some(home) = std::env::var_os("HOME") {
            out.push(
                PathBuf::from(home).join("Applications/LibreOffice.app/Contents/MacOS/soffice"),
            );
        }
    } else if cfg!(windows) {
        // soffice.com waits for the conversion and passes its output on.
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(var) {
                let program = PathBuf::from(root).join(r"LibreOffice\program");
                out.push(program.join("soffice.com"));
                out.push(program.join("soffice.exe"));
            }
        }
    } else {
        for dir in ["/usr/lib/libreoffice", "/usr/lib64/libreoffice"] {
            out.push(Path::new(dir).join("program/soffice.bin"));
        }
        // /opt/libreoffice25.8 and the like, newest first.
        let mut opt: Vec<PathBuf> = std::fs::read_dir("/opt")
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("libreoffice"))
            })
            .collect();
        opt.sort();
        out.extend(opt.into_iter().rev().map(|p| p.join("program/soffice.bin")));
    }
    out
}

/// Mach services LibreOffice may look up on macOS. Headless `soffice` still
/// registers as an application (window server, LaunchServices) and aborts if
/// it can't. The list is what `soffice --headless --convert-to` looked up for
/// Writer, Calc and Impress documents under a logging profile, minus every
/// service that reaches user data or other apps, which stay denied and are
/// not needed: the clipboard (`pasteboard.1`, `pbs.fetch_services`), Apple
/// Events, recent documents (`sharedfilelistd`), security-scoped bookmarks,
/// Siri and Shortcuts donations, analytics, the Dock, distributed
/// notifications, window restoration, privacy prompts (`tccd`) and
/// LaunchServices writes (`lsd.modifydb`, which could change file
/// associations).
const SOFFICE_MACH: &[&str] = &[
    "com.apple.CARenderServer",
    "com.apple.CoreServices.coreservicesd",
    "com.apple.MenuBarAgent.systemservices",
    "com.apple.ViewBridgeAuxiliary",
    "com.apple.cfprefsd.agent",
    "com.apple.cfprefsd.daemon",
    "com.apple.coreservices.launchservicesd",
    "com.apple.cvmsServ",
    "com.apple.fonts",
    "com.apple.hiservices-xpcservice",
    "com.apple.logd",
    "com.apple.lsd.mapdb",
    "com.apple.runningboard",
    "com.apple.system.notification_center",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.touchbarserver.mig",
    "com.apple.window_proxies",
    "com.apple.windowmanager.server",
    "com.apple.windowserver.active",
];

/// LibreOffice's single-instance pipe: `/tmp/OSL_PIPE_<uid>_SingleOfficeIPC_<hash>`
/// (the hash is of the per-job profile path, so jobs never share one).
const SOFFICE_PIPE: (&str, &str) = ("/private/tmp", r"OSL_PIPE_[0-9]+_SingleOfficeIPC_[0-9a-f]+");

#[derive(Debug, Clone, Default)]
pub struct Engines {
    /// Engine names that can run here.
    pub installed: BTreeSet<String>,
    /// External engines' sandbox grants, by engine name.
    pub grants: BTreeMap<String, Grant>,
}

impl Engines {
    pub fn discover(python: &Path) -> Engines {
        let mut e = Engines::default();
        e.installed.extend(python_modules(python));
        if let (Some(ffmpeg), Some(ffprobe)) = (find_bin("ffmpeg"), find_bin("ffprobe")) {
            // A binary whose install folder can't be shared safely is treated
            // as not installed.
            if let Some(grant) = binary_grant(&[("ffmpeg", &ffmpeg), ("ffprobe", &ffprobe)]) {
                e.installed.insert("ffmpeg".into());
                e.grants.insert("ffmpeg".into(), grant);
            }
        }
        let soffice = soffice_candidates()
            .into_iter()
            .find(|p| p.is_file())
            .and_then(|p| p.canonicalize().ok());
        if let Some(grant) =
            soffice.and_then(|real| libreoffice_grant(&converter_sandbox::plain_path(&real)))
        {
            e.installed.insert("libreoffice".into());
            e.grants.insert("libreoffice".into(), grant);
        }
        let gs = if cfg!(windows) { "gswin64c" } else { "gs" };
        for (engine, env, bin) in [
            ("ghostscript", "gs", gs),
            ("tesseract", "tesseract", "tesseract"),
        ] {
            if let Some(grant) = find_bin(bin).and_then(|path| binary_grant(&[(env, &path)])) {
                e.installed.insert(engine.into());
                e.grants.insert(engine.into(), grant);
            }
        }
        e
    }

    /// Each adapter's grant: the union of its external engines' grants.
    pub fn adapter_grants(&self, registry: &Registry) -> BTreeMap<String, Grant> {
        let mut out = BTreeMap::new();
        for a in &registry.adapters {
            let mut grant = Grant::default();
            let mut any = false;
            for engine in &a.manifest.requires {
                if let Some(g) = self.grants.get(engine) {
                    grant.merge(g);
                    any = true;
                }
            }
            // Each extra process the worker starts is another Python (a
            // verifier). From a Windows virtualenv that is two processes, the
            // launcher and the interpreter, and Windows enforces the count.
            grant.processes += a.manifest.limits.processes.saturating_sub(1) * PYTHON_PROCESSES;
            if any || grant.processes > 0 {
                out.insert(a.manifest.id.clone(), grant);
            }
        }
        out
    }
}

/// Processes one extra Python process may need: a virtualenv launcher and
/// the interpreter it starts.
const PYTHON_PROCESSES: u32 = 2;

/// How long the interpreter gets to list its modules. Startup waits on this;
/// a broken or hung interpreter (a stuck network drive, a prompt) must not
/// freeze the app.
const PYTHON_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Which engine modules the worker interpreter can import. If it doesn't
/// answer in time, none: the app then offers no Python conversions.
fn python_modules(python: &Path) -> BTreeSet<String> {
    python_modules_within(python, PYTHON_PROBE_TIMEOUT)
}

fn python_modules_within(python: &Path, timeout: Duration) -> BTreeSet<String> {
    let modules: Vec<&str> = PYTHON_ENGINES.iter().map(|(_, m)| *m).collect();
    let script = format!(
        "import importlib.util as u\nfor m in {modules:?}:\n    print(m, u.find_spec(m) is not None)"
    );
    let Ok(mut child) = Command::new(python)
        .args(["-I", "-c", &script])
        .env_clear()
        .envs(converter_sandbox::base_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return BTreeSet::new();
    };
    // The answer is a few hundred bytes, well under a pipe buffer, so waiting
    // before reading can't block the child.
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return BTreeSet::new(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return BTreeSet::new();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut text);
    }
    let found: BTreeSet<String> = text
        .lines()
        .filter_map(|l| l.strip_suffix(" True").map(str::to_owned))
        .collect();
    PYTHON_ENGINES
        .iter()
        .filter(|(_, m)| found.contains(*m))
        .map(|(e, _)| (*e).to_owned())
        .collect()
}

/// An engine binary on `PATH` or in the usual folders, resolved to the real
/// file. Anything that resolves to a different program (Snap's `snap`
/// launcher, say) can't run in the sandbox and is skipped.
fn find_bin(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    path_dirs(std::env::var_os("PATH").as_deref())
        .into_iter()
        .chain(bin_dirs(name))
        .map(|d| d.join(&file))
        .filter(|p| p.is_file())
        .filter_map(|p| scoop_shim_target(&p).unwrap_or(p).canonicalize().ok())
        .map(|p| converter_sandbox::plain_path(&p))
        .find(|p| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(name)))
}

/// `PATH` entries that name one place. An empty entry or a relative one means
/// "the current directory" to a shell, so a hostile folder the app happens to
/// run from could supply the engine; both are ignored.
fn path_dirs(path: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    path.map(|p| {
        std::env::split_paths(p)
            .filter(|d| d.is_absolute())
            .collect()
    })
    .unwrap_or_default()
}

/// Scoop's shims start the real binary named in `<name>.shim` beside them.
fn scoop_shim_target(path: &Path) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let text = std::fs::read_to_string(path.with_extension("shim")).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("path"))?;
    let target = line
        .trim_start()
        .strip_prefix('=')?
        .trim()
        .trim_matches('"');
    Some(PathBuf::from(target))
}

/// Read the binary's install (for Homebrew, every keg: dependencies live in
/// sibling kegs), execute only the named binaries. `None` when its install
/// can't be shared safely (see [`install_root`]).
fn binary_grant(bins: &[(&str, &Path)]) -> Option<Grant> {
    let mut g = Grant::default();
    for (name, path) in bins {
        for root in install_root(path)? {
            if !g.read.contains(&root) {
                g.read.push(root);
            }
        }
        g.exec.push(path.to_path_buf());
        g.env.push((
            format!("LFCS_BIN_{}", name.to_ascii_uppercase()),
            OsString::from(path.as_os_str()),
        ));
    }
    // One at a time: FFmpeg, then FFprobe.
    g.processes = 1;
    Some(g)
}

/// Where Homebrew keeps kegs; the Cellar there holds every formula, and a
/// binary's libraries live in sibling kegs.
const BREW_PREFIXES: &[&str] = &["/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"];

/// The folders to share read-only so `bin` can load its libraries, from a
/// short list of known layouts: the Homebrew Cellar, `/usr`, `/opt/<name>`,
/// `C:\Program Files\<name>` and an app bundle. Any other binary gets its own
/// folder plus a sibling `lib`. `None` when that would be, or contain, the
/// home folder or `/`: a `~/bin/ffmpeg` in `PATH` must not share the home
/// folder with a sandboxed worker (a binary found that way is treated as not
/// installed).
fn install_root(bin: &Path) -> Option<Vec<PathBuf>> {
    install_root_in(bin, converter_sandbox::home_dir().as_deref())
}

fn install_root_in(bin: &Path, home: Option<&Path>) -> Option<Vec<PathBuf>> {
    let known = |dir: &str| bin.parent().is_some_and(|p| p == Path::new(dir));
    let roots: Vec<PathBuf> = if let Some(cellar) = bin.ancestors().find(|a| {
        a.file_name().is_some_and(|n| n == "Cellar")
            && a.parent()
                .is_some_and(|p| BREW_PREFIXES.iter().any(|b| p == Path::new(b)))
    }) {
        vec![cellar.to_path_buf()]
    } else if let Some(app) = bin
        .ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
    {
        vec![app.to_path_buf()]
    } else if known("/usr/bin") || known("/usr/sbin") {
        // Libraries live in /usr/lib.
        vec![PathBuf::from("/usr")]
    } else if known("/usr/local/bin") || known("/usr/local/sbin") {
        vec![PathBuf::from("/usr/local")]
    } else if let Some(opt) = bin.ancestors().find(|a| {
        a.parent().is_some_and(|p| p == Path::new("/opt"))
            && a.file_name().is_some_and(|n| n != "homebrew")
    }) {
        vec![opt.to_path_buf()]
    } else if let Some(app) = bin.ancestors().find(|a| {
        a.parent().and_then(Path::file_name).is_some_and(|n| {
            n.eq_ignore_ascii_case("Program Files") || n.eq_ignore_ascii_case("Program Files (x86)")
        })
    }) {
        vec![app.to_path_buf()]
    } else {
        let dir = bin.parent()?;
        let mut v = vec![dir.to_path_buf()];
        if let Some(lib) = dir.parent().map(|p| p.join("lib")).filter(|l| l.is_dir()) {
            v.push(lib);
        }
        v
    };
    roots
        .iter()
        .all(|r| !converter_sandbox::too_broad(r, home))
        .then_some(roots)
}

fn libreoffice_grant(soffice: &Path) -> Option<Grant> {
    // …/LibreOffice.app/Contents/MacOS/soffice → …/LibreOffice.app
    let read = install_root(soffice)?;
    let mut g = Grant {
        read,
        exec: vec![soffice.parent().unwrap_or(soffice).to_path_buf()],
        env: vec![(
            "LFCS_BIN_SOFFICE".into(),
            OsString::from(soffice.as_os_str()),
        )],
        // soffice.com starts soffice.bin, which may start a helper.
        processes: 3,
        ..Default::default()
    };
    if cfg!(windows) {
        // Fonts in C:\Windows\Fonts are readable by every AppContainer.
    } else if cfg!(target_os = "macos") {
        g.mach = SOFFICE_MACH.iter().map(|s| (*s).to_owned()).collect();
        g.listen = vec![(PathBuf::from(SOFFICE_PIPE.0), SOFFICE_PIPE.1.to_owned())];
        if Path::new("/Library/Fonts").is_dir() {
            g.read.push(PathBuf::from("/Library/Fonts"));
        }
    } else {
        for dir in [
            "/usr/share/fonts",
            "/etc/fonts",
            "/etc/libreoffice",
            "/usr/lib/libreoffice",
        ] {
            if Path::new(dir).is_dir() {
                g.read.push(PathBuf::from(dir));
            }
        }
    }
    Some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const HOME: &str = "/Users/me";

    #[cfg(unix)]
    fn roots(bin: &str) -> Option<Vec<PathBuf>> {
        install_root_in(Path::new(bin), Some(Path::new(HOME)))
    }

    // Unix layouts: these paths aren't absolute on Windows, which has its own.
    #[cfg(unix)]
    #[test]
    fn homebrew_kegs_share_one_read_root() {
        assert_eq!(
            roots("/opt/homebrew/Cellar/ffmpeg/7.1.1_4/bin/ffmpeg").unwrap(),
            [PathBuf::from("/opt/homebrew/Cellar")]
        );
        assert_eq!(roots("/usr/bin/ffmpeg").unwrap(), [PathBuf::from("/usr")]);
        // A folder that merely calls itself Cellar isn't Homebrew's.
        assert_eq!(
            roots("/Users/me/Cellar/x/bin/x").unwrap(),
            [PathBuf::from("/Users/me/Cellar/x/bin")]
        );
    }

    // Unix layouts: these paths aren't absolute on Windows, which has its own.
    #[cfg(unix)]
    #[test]
    fn a_binary_in_the_home_folder_never_grants_home() {
        // Typical: ~/bin is on PATH.
        assert_eq!(
            roots("/Users/me/bin/ffmpeg").unwrap(),
            [PathBuf::from("/Users/me/bin")]
        );
        // Directly in home: the binary's folder would be home itself.
        assert!(roots("/Users/me/ffmpeg").is_none());
        // Directly under /: its folder is `/`.
        assert!(roots("/ffmpeg").is_none());
        // /Users/ffmpeg would grant every user's files.
        assert!(roots("/Users/ffmpeg").is_none());
        for bin in [
            "/Users/me/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/opt/libreoffice25.8/program/soffice.bin",
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        ] {
            for r in roots(bin).unwrap() {
                assert!(
                    !converter_sandbox::too_broad(&r, Some(Path::new(HOME))),
                    "{r:?}"
                );
            }
        }
    }

    // Unix layouts: these paths aren't absolute on Windows, which has its own.
    #[cfg(unix)]
    #[test]
    fn known_layouts_grant_their_install() {
        assert_eq!(
            roots("/usr/local/bin/gs").unwrap(),
            [PathBuf::from("/usr/local")]
        );
        assert_eq!(
            roots("/opt/libreoffice25.8/program/soffice.bin").unwrap(),
            [PathBuf::from("/opt/libreoffice25.8")]
        );
        assert_eq!(
            roots("/Applications/LibreOffice.app/Contents/MacOS/soffice").unwrap(),
            [PathBuf::from("/Applications/LibreOffice.app")]
        );
        // Homebrew's own prefix isn't an /opt/<name> install: just the
        // binary's folder and, if the machine has it, the sibling lib.
        let r = roots("/opt/homebrew/bin/ffmpeg").unwrap();
        assert_eq!(r[0], Path::new("/opt/homebrew/bin"));
        assert!(r.len() == 1 || r[1] == Path::new("/opt/homebrew/lib"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_program_files_grants_the_app_folder() {
        let r = install_root_in(
            Path::new(r"C:\Program Files\LibreOffice\program\soffice.com"),
            Some(Path::new(r"C:\Users\me")),
        )
        .unwrap();
        assert_eq!(r, [PathBuf::from(r"C:\Program Files\LibreOffice")]);
        assert!(install_root_in(
            Path::new(r"C:\Users\me\ffmpeg.exe"),
            Some(Path::new(r"C:\Users\me"))
        )
        .is_none());
    }

    // Unix layouts: these paths aren't absolute on Windows, which has its own.
    #[cfg(unix)]
    #[test]
    fn grants_name_the_binaries() {
        let g = binary_grant(&[(
            "ffmpeg",
            Path::new("/opt/homebrew/Cellar/ffmpeg/7/bin/ffmpeg"),
        )])
        .unwrap();
        assert_eq!(
            g.exec,
            vec![PathBuf::from("/opt/homebrew/Cellar/ffmpeg/7/bin/ffmpeg")]
        );
        assert_eq!(g.env[0].0, "LFCS_BIN_FFMPEG");
    }

    #[test]
    fn path_entries_must_be_absolute() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let abs = if cfg!(windows) {
            r"C:\tools"
        } else {
            "/usr/bin"
        };
        // Empty entries (leading, middle, trailing) and relative ones.
        let value = format!("{sep}rel/bin{sep}{abs}{sep}{sep}.{sep}");
        assert_eq!(
            path_dirs(Some(std::ffi::OsStr::new(&value))),
            [PathBuf::from(abs)]
        );
        assert!(path_dirs(None).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_interpreter_reports_no_engines_instead_of_hanging() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let fake = t.path().join("python");
        // exec: the sleep is the child, so killing it frees the pipe.
        std::fs::write(&fake, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        let found = python_modules_within(&fake, Duration::from_millis(300));
        assert!(found.is_empty());
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:?}",
            start.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_working_interpreter_is_still_read() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let fake = t.path().join("python");
        std::fs::write(&fake, "#!/bin/sh\necho 'pikepdf True'\necho 'PIL False'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = python_modules_within(&fake, Duration::from_secs(10));
        assert_eq!(
            found,
            BTreeSet::from(["pikepdf".to_owned(), "qpdf".to_owned()])
        );
    }
}
