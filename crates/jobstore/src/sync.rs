//! Cloud-sync folder detection (spec section 10.4 and 10.6, blueprint 6.5).
//!
//! **Detection is best effort.** It uses each provider's documented
//! locations and environment variables:
//!
//! | Provider | Windows | macOS | Linux |
//! |---|---|---|---|
//! | OneDrive | `OneDrive`, `OneDriveConsumer`, `OneDriveCommercial` variables | `~/Library/CloudStorage/OneDrive*` | none |
//! | Dropbox | `info.json` in `%APPDATA%\Dropbox` or `%LOCALAPPDATA%\Dropbox`; `~\Dropbox` | `~/.dropbox/info.json`; `~/Library/CloudStorage/Dropbox*`; `~/Dropbox` | `~/.dropbox/info.json`; `~/Dropbox` |
//! | iCloud Drive | `~\iCloudDrive` | `~/Library/Mobile Documents` | none |
//! | Google Drive | `~\Google Drive`, `~\My Drive` | `~/Library/CloudStorage/GoogleDrive*`, `~/Google Drive`, `~/My Drive` | none |
//!
//! Any other folder under macOS `~/Library/CloudStorage` belongs to a
//! File Provider sync app and is reported as [`SyncProvider::Other`].
//!
//! Known gaps: Google Drive's virtual drive letter on Windows, iCloud's
//! "Desktop and Documents" option on macOS, folders moved after the provider
//! wrote its settings, and providers not listed here.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::fsutil::{is_within, normalize};

/// A cloud-sync provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncProvider {
    OneDrive,
    Dropbox,
    ICloudDrive,
    GoogleDrive,
    /// Another macOS File Provider app under `~/Library/CloudStorage`.
    Other,
}

/// Which OS's rules to apply. Separate from the build target so tests can
/// exercise every OS's rules on any machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOs {
    Windows,
    MacOs,
    Linux,
}

impl SyncOs {
    pub fn current() -> Self {
        if cfg!(windows) {
            SyncOs::Windows
        } else if cfg!(target_os = "macos") {
            SyncOs::MacOs
        } else {
            SyncOs::Linux
        }
    }

    fn case_insensitive(self) -> bool {
        // NTFS and the default APFS volume are case-insensitive.
        !matches!(self, SyncOs::Linux)
    }
}

/// Where to look for sync folders.
#[derive(Debug, Clone)]
pub struct SyncEnv {
    pub os: SyncOs,
    pub home: Option<PathBuf>,
    /// Environment variables that hold paths: `OneDrive`, `OneDriveConsumer`,
    /// `OneDriveCommercial`, `APPDATA` and `LOCALAPPDATA`.
    pub vars: Vec<(String, PathBuf)>,
}

const PATH_VARS: [&str; 5] = [
    "OneDrive",
    "OneDriveConsumer",
    "OneDriveCommercial",
    "APPDATA",
    "LOCALAPPDATA",
];

impl SyncEnv {
    /// The current user's environment.
    pub fn current() -> Self {
        let vars = PATH_VARS
            .iter()
            .filter_map(|&k| {
                let v = std::env::var_os(k)?;
                (!v.is_empty()).then(|| (k.to_owned(), PathBuf::from(v)))
            })
            .collect();
        SyncEnv {
            os: SyncOs::current(),
            home: dirs::home_dir(),
            vars,
        }
    }

    fn var(&self, key: &str) -> Option<&Path> {
        self.vars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_path())
    }
}

/// The sync folders found on this machine.
#[derive(Debug, Clone)]
pub struct SyncRoots {
    roots: Vec<(PathBuf, SyncProvider)>,
    case_insensitive: bool,
}

impl SyncRoots {
    /// Discovers sync folders for the current user.
    pub fn discover() -> Self {
        Self::discover_in(&SyncEnv::current())
    }

    /// No sync folders, for tests and for callers that opt out.
    pub fn none() -> Self {
        SyncRoots {
            roots: Vec::new(),
            case_insensitive: SyncOs::current().case_insensitive(),
        }
    }

    /// Discovers sync folders using the given environment.
    pub fn discover_in(env: &SyncEnv) -> Self {
        let mut roots: Vec<(PathBuf, SyncProvider)> = Vec::new();
        let mut add = |p: PathBuf, provider| roots.push((p, provider));

        match env.os {
            SyncOs::Windows => {
                for k in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
                    if let Some(p) = env.var(k) {
                        add(p.to_path_buf(), SyncProvider::OneDrive);
                    }
                }
                for k in ["APPDATA", "LOCALAPPDATA"] {
                    if let Some(base) = env.var(k) {
                        for p in dropbox_info_paths(&base.join("Dropbox").join("info.json")) {
                            add(p, SyncProvider::Dropbox);
                        }
                    }
                }
                if let Some(home) = &env.home {
                    add(home.join("Dropbox"), SyncProvider::Dropbox);
                    add(home.join("iCloudDrive"), SyncProvider::ICloudDrive);
                    add(home.join("Google Drive"), SyncProvider::GoogleDrive);
                    add(home.join("My Drive"), SyncProvider::GoogleDrive);
                }
            }
            SyncOs::MacOs => {
                if let Some(home) = &env.home {
                    for p in dropbox_info_paths(&home.join(".dropbox").join("info.json")) {
                        add(p, SyncProvider::Dropbox);
                    }
                    add(home.join("Dropbox"), SyncProvider::Dropbox);
                    add(
                        home.join("Library").join("Mobile Documents"),
                        SyncProvider::ICloudDrive,
                    );
                    add(home.join("Google Drive"), SyncProvider::GoogleDrive);
                    add(home.join("My Drive"), SyncProvider::GoogleDrive);
                    let cloud_storage = home.join("Library").join("CloudStorage");
                    // The folder itself holds only provider folders.
                    add(cloud_storage.clone(), SyncProvider::Other);
                    if let Ok(entries) = std::fs::read_dir(&cloud_storage) {
                        for e in entries.flatten() {
                            let name = e.file_name().to_string_lossy().to_lowercase();
                            let provider = if name.starts_with("onedrive") {
                                SyncProvider::OneDrive
                            } else if name.starts_with("dropbox") {
                                SyncProvider::Dropbox
                            } else if name.starts_with("googledrive") {
                                SyncProvider::GoogleDrive
                            } else {
                                SyncProvider::Other
                            };
                            add(e.path(), provider);
                        }
                    }
                }
            }
            SyncOs::Linux => {
                if let Some(home) = &env.home {
                    for p in dropbox_info_paths(&home.join(".dropbox").join("info.json")) {
                        add(p, SyncProvider::Dropbox);
                    }
                    add(home.join("Dropbox"), SyncProvider::Dropbox);
                }
            }
        }

        let mut roots: Vec<(PathBuf, SyncProvider)> = roots
            .into_iter()
            .filter(|(p, _)| p.is_absolute())
            .map(|(p, provider)| (normalize(&p), provider))
            .collect();
        // Most specific first, so a provider folder inside CloudStorage wins
        // over the CloudStorage folder itself.
        roots.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        roots.dedup_by(|a, b| a.0 == b.0);

        SyncRoots {
            roots,
            case_insensitive: env.os.case_insensitive(),
        }
    }

    /// Builds a set from explicit folders.
    pub fn from_roots(roots: Vec<(PathBuf, SyncProvider)>, os: SyncOs) -> Self {
        let mut roots: Vec<_> = roots
            .into_iter()
            .map(|(p, provider)| (normalize(&p), provider))
            .collect();
        roots.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        SyncRoots {
            roots,
            case_insensitive: os.case_insensitive(),
        }
    }

    /// The provider whose folder contains `path`, if any. `path` need not
    /// exist yet.
    pub fn provider_for(&self, path: &Path) -> Option<SyncProvider> {
        if self.roots.is_empty() {
            return None;
        }
        let path = normalize(path);
        self.roots
            .iter()
            .find(|(root, _)| is_within(&path, root, self.case_insensitive))
            .map(|(_, provider)| *provider)
    }
}

/// Reads Dropbox's documented `info.json`, which lists the personal and
/// business folder paths.
fn dropbox_info_paths(info: &Path) -> Vec<PathBuf> {
    // The file is a few hundred bytes; refuse anything absurd.
    let Ok(meta) = std::fs::metadata(info) else {
        return Vec::new();
    };
    if meta.len() > 64 * 1024 {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(info) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(accounts) = value.as_object() else {
        return Vec::new();
    };
    accounts
        .values()
        .filter_map(|a| a.get("path")?.as_str())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: SyncOs, home: &Path, vars: &[(&str, PathBuf)]) -> SyncEnv {
        SyncEnv {
            os,
            home: Some(home.to_path_buf()),
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn windows_onedrive_variable_and_dropbox_info() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let appdata = home.join("AppData").join("Roaming");
        std::fs::create_dir_all(appdata.join("Dropbox")).unwrap();
        let team = home.join("Team Box");
        std::fs::write(
            appdata.join("Dropbox").join("info.json"),
            serde_json::json!({"business": {"path": team, "host": 1}}).to_string(),
        )
        .unwrap();
        let roots = SyncRoots::discover_in(&env(
            SyncOs::Windows,
            home,
            &[
                ("OneDrive", home.join("OneDrive - Contoso")),
                ("APPDATA", appdata),
            ],
        ));
        assert_eq!(
            roots.provider_for(&home.join("OneDrive - Contoso").join("x.pdf")),
            Some(SyncProvider::OneDrive)
        );
        assert_eq!(
            roots.provider_for(&team.join("deep").join("x.pdf")),
            Some(SyncProvider::Dropbox)
        );
        assert_eq!(
            roots.provider_for(&home.join("iCloudDrive")),
            Some(SyncProvider::ICloudDrive)
        );
        assert_eq!(roots.provider_for(&home.join("Documents")), None);
        assert_eq!(roots.provider_for(&home.join("OneDrive2")), None);
    }

    #[test]
    fn macos_cloud_storage_and_icloud() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let cs = home.join("Library").join("CloudStorage");
        for d in ["OneDrive-Personal", "GoogleDrive-a@b.c", "Box-Box"] {
            std::fs::create_dir_all(cs.join(d)).unwrap();
        }
        let roots = SyncRoots::discover_in(&env(SyncOs::MacOs, home, &[]));
        assert_eq!(
            roots.provider_for(&cs.join("OneDrive-Personal").join("a")),
            Some(SyncProvider::OneDrive)
        );
        assert_eq!(
            roots.provider_for(&cs.join("GoogleDrive-a@b.c").join("My Drive")),
            Some(SyncProvider::GoogleDrive)
        );
        assert_eq!(
            roots.provider_for(&cs.join("Box-Box")),
            Some(SyncProvider::Other)
        );
        assert_eq!(
            roots.provider_for(
                &home
                    .join("Library")
                    .join("Mobile Documents")
                    .join("com~apple~CloudDocs")
            ),
            Some(SyncProvider::ICloudDrive)
        );
        assert_eq!(
            roots.provider_for(&home.join("Library").join("Application Support")),
            None
        );
    }

    #[test]
    fn linux_dropbox_info_and_garbage_info() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".dropbox")).unwrap();
        std::fs::write(home.join(".dropbox").join("info.json"), "{not json").unwrap();
        let roots = SyncRoots::discover_in(&env(SyncOs::Linux, home, &[]));
        // Falls back to the default folder.
        assert_eq!(
            roots.provider_for(&home.join("Dropbox").join("f")),
            Some(SyncProvider::Dropbox)
        );
        assert_eq!(roots.provider_for(&home.join(".cache")), None);
    }

    #[test]
    fn case_rules_follow_the_os() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Sync");
        let upper = tmp.path().join("SYNC").join("f");
        let win = SyncRoots::from_roots(vec![(root.clone(), SyncProvider::Other)], SyncOs::Windows);
        let lin = SyncRoots::from_roots(vec![(root, SyncProvider::Other)], SyncOs::Linux);
        assert_eq!(win.provider_for(&upper), Some(SyncProvider::Other));
        if !cfg!(any(windows, target_os = "macos")) {
            assert_eq!(lin.provider_for(&upper), None);
        }
    }
}
