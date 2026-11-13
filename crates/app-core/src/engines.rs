//! Which engines this installation has (ADR 0003).
//!
//! Python engines are importable modules of the worker interpreter. External
//! engines (FFmpeg, LibreOffice) are binaries found in the usual install
//! locations; nothing is downloaded. The result decides which conversions are
//! offered (`Availability`) and which sandbox grants each adapter gets.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

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
            e.installed.insert("ffmpeg".into());
            e.grants.insert(
                "ffmpeg".into(),
                binary_grant(&[("ffmpeg", &ffmpeg), ("ffprobe", &ffprobe)]),
            );
        }
        let soffice = soffice_candidates()
            .into_iter()
            .find(|p| p.is_file())
            .and_then(|p| p.canonicalize().ok());
        if let Some(real) = soffice {
            e.installed.insert("libreoffice".into());
            e.grants.insert(
                "libreoffice".into(),
                libreoffice_grant(&converter_sandbox::plain_path(&real)),
            );
        }
        let gs = if cfg!(windows) { "gswin64c" } else { "gs" };
        for (engine, env, bin) in [
            ("ghostscript", "gs", gs),
            ("tesseract", "tesseract", "tesseract"),
        ] {
            if let Some(path) = find_bin(bin) {
                e.installed.insert(engine.into());
                e.grants
                    .insert(engine.into(), binary_grant(&[(env, &path)]));
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
            grant.processes += a.manifest.limits.processes.saturating_sub(1);
            if any || grant.processes > 0 {
                out.insert(a.manifest.id.clone(), grant);
            }
        }
        out
    }
}

/// Which engine modules the worker interpreter can import.
fn python_modules(python: &Path) -> BTreeSet<String> {
    let modules: Vec<&str> = PYTHON_ENGINES.iter().map(|(_, m)| *m).collect();
    let script = format!(
        "import importlib.util as u\nfor m in {modules:?}:\n    print(m, u.find_spec(m) is not None)"
    );
    let Ok(out) = Command::new(python)
        .args(["-I", "-c", &script])
        .env_clear()
        .envs(converter_sandbox::base_env())
        .output()
    else {
        return BTreeSet::new();
    };
    let found: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
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
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    path_dirs
        .into_iter()
        .chain(bin_dirs(name))
        .map(|d| d.join(&file))
        .filter(|p| p.is_file())
        .filter_map(|p| scoop_shim_target(&p).unwrap_or(p).canonicalize().ok())
        .map(|p| converter_sandbox::plain_path(&p))
        .find(|p| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(name)))
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
/// sibling kegs), execute only the named binaries.
fn binary_grant(bins: &[(&str, &Path)]) -> Grant {
    let mut g = Grant::default();
    for (name, path) in bins {
        let root = install_root(path);
        if !g.read.contains(&root) {
            g.read.push(root);
        }
        g.exec.push(path.to_path_buf());
        g.env.push((
            format!("LFCS_BIN_{}", name.to_ascii_uppercase()),
            OsString::from(path.as_os_str()),
        ));
    }
    // One at a time: FFmpeg, then FFprobe.
    g.processes = 1;
    g
}

fn install_root(bin: &Path) -> PathBuf {
    let s = bin.to_string_lossy();
    if let Some(i) = s.find("/Cellar/") {
        return PathBuf::from(&s[..i + "/Cellar".len()]);
    }
    // e.g. /usr/bin/ffmpeg → /usr (libraries live in /usr/lib), or
    // C:\ffmpeg\bin\ffmpeg.exe → C:\ffmpeg.
    bin.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn libreoffice_grant(soffice: &Path) -> Grant {
    // …/LibreOffice.app/Contents/MacOS/soffice → …/LibreOffice.app
    let app = soffice
        .ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf);
    let root = app.clone().unwrap_or_else(|| install_root(soffice));
    let mut g = Grant {
        read: vec![root],
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
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homebrew_kegs_share_one_read_root() {
        assert_eq!(
            install_root(Path::new("/opt/homebrew/Cellar/ffmpeg/7.1.1_4/bin/ffmpeg")),
            Path::new("/opt/homebrew/Cellar")
        );
        assert_eq!(
            install_root(Path::new("/usr/bin/ffmpeg")),
            Path::new("/usr")
        );
    }

    #[test]
    fn grants_name_the_binaries() {
        let g = binary_grant(&[(
            "ffmpeg",
            Path::new("/opt/homebrew/Cellar/ffmpeg/7/bin/ffmpeg"),
        )]);
        assert_eq!(
            g.exec,
            vec![PathBuf::from("/opt/homebrew/Cellar/ffmpeg/7/bin/ffmpeg")]
        );
        assert_eq!(g.env[0].0, "LFCS_BIN_FFMPEG");
    }
}
