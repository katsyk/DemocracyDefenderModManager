//! Generates and (re)registers the native-messaging host manifest so the
//! browser extension can find and launch `ddmm` in host mode. Registration
//! is idempotent and re-run on every launch, because the exe may have
//! moved (a new portable build, an AppImage re-download, ...).
//!
//! See `docs/development/bridge-protocol.md` for the identifiers below.

use std::path::{Path, PathBuf};

use crate::bridge::allowlist::{CHROME_EXTENSION_ID, FIREFOX_EXTENSION_ID};

/// The native messaging host name both browsers look the manifest up by.
pub const HOST_NAME: &str = "io.github.katsyk.ddmm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    Chromium,
    Firefox,
}

/// One browser DDMM can register itself with. `registry_hive` is the
/// Chromium browser's own `HKCU\Software\<vendor>\<Product>` prefix used on
/// Windows (`None` for browsers that read another vendor's key, e.g. Brave
/// reading Chrome's); `config_dir` is the Linux config-directory name under
/// `~/.config` (`None` for Firefox, which uses its own fixed path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserTarget {
    pub id: &'static str,
    pub display_name: &'static str,
    pub kind: ManifestKind,
    pub registry_hive: Option<&'static str>,
    pub config_dir: Option<&'static str>,
}

pub const BROWSERS: &[BrowserTarget] = &[
    BrowserTarget {
        id: "chrome",
        display_name: "Google Chrome",
        kind: ManifestKind::Chromium,
        registry_hive: Some("Software\\Google\\Chrome\\NativeMessagingHosts"),
        config_dir: Some("google-chrome"),
    },
    BrowserTarget {
        id: "chromium",
        display_name: "Chromium",
        kind: ManifestKind::Chromium,
        // Windows Chromium reads the same key as Chrome; nothing extra to add there.
        registry_hive: None,
        config_dir: Some("chromium"),
    },
    BrowserTarget {
        id: "edge",
        display_name: "Microsoft Edge",
        kind: ManifestKind::Chromium,
        registry_hive: Some("Software\\Microsoft\\Edge\\NativeMessagingHosts"),
        config_dir: Some("microsoft-edge"),
    },
    BrowserTarget {
        id: "brave",
        display_name: "Brave",
        kind: ManifestKind::Chromium,
        // Brave reads Chrome's NativeMessagingHosts key on Windows.
        registry_hive: None,
        config_dir: Some("BraveSoftware/Brave-Browser"),
    },
    BrowserTarget {
        id: "vivaldi",
        display_name: "Vivaldi",
        kind: ManifestKind::Chromium,
        // Vivaldi also reads Chrome's key on Windows.
        registry_hive: None,
        config_dir: Some("vivaldi"),
    },
    BrowserTarget {
        id: "firefox",
        display_name: "Firefox",
        kind: ManifestKind::Firefox,
        registry_hive: Some("Software\\Mozilla\\NativeMessagingHosts"),
        config_dir: None,
    },
];

fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The manifest content for a Chromium-family browser.
pub fn chromium_manifest_json(exe_path: &Path) -> String {
    format!(
        "{{\n  \"name\": \"{}\",\n  \"description\": \"Democracy Defender Mod Manager bridge\",\n  \"path\": \"{}\",\n  \"type\": \"stdio\",\n  \"allowed_origins\": [\"chrome-extension://{}/\"]\n}}\n",
        HOST_NAME,
        escape_json_string(&exe_path.to_string_lossy()),
        CHROME_EXTENSION_ID,
    )
}

/// The manifest content for Firefox.
pub fn firefox_manifest_json(exe_path: &Path) -> String {
    format!(
        "{{\n  \"name\": \"{}\",\n  \"description\": \"Democracy Defender Mod Manager bridge\",\n  \"path\": \"{}\",\n  \"type\": \"stdio\",\n  \"allowed_extensions\": [\"{}\"]\n}}\n",
        HOST_NAME,
        escape_json_string(&exe_path.to_string_lossy()),
        FIREFOX_EXTENSION_ID,
    )
}

pub fn manifest_json(target: &BrowserTarget, exe_path: &Path) -> String {
    match target.kind {
        ManifestKind::Chromium => chromium_manifest_json(exe_path),
        ManifestKind::Firefox => firefox_manifest_json(exe_path),
    }
}

/// Where the manifest file for `target` lives on Linux, under `home`.
/// Firefox always reads `~/.mozilla/native-messaging-hosts/<host>.json`
/// regardless of profile.
pub fn linux_manifest_path(home: &Path, target: &BrowserTarget) -> Option<PathBuf> {
    match target.kind {
        ManifestKind::Firefox => Some(home.join(".mozilla/native-messaging-hosts").join(format!("{HOST_NAME}.json"))),
        ManifestKind::Chromium => {
            let dir = target.config_dir?;
            Some(
                home.join(".config")
                    .join(dir)
                    .join("NativeMessagingHosts")
                    .join(format!("{HOST_NAME}.json")),
            )
        }
    }
}

/// Where the manifest file for `target` lives on Windows -- unlike Linux,
/// the *registry value* points at this path; the file itself can live
/// anywhere writable, so it goes next to the exe's data folder.
pub fn windows_manifest_path(base_path: &Path, target: &BrowserTarget) -> PathBuf {
    base_path
        .join("native-messaging")
        .join(format!("{}-{}.json", HOST_NAME, target.id))
}

#[derive(Debug, Clone)]
pub struct RegistrationOutcome {
    pub browser_id: &'static str,
    pub registered: bool,
    pub detail: String,
}

/// Write every applicable manifest file for the current platform and (on
/// Windows) point each browser's registry key at it. Safe to call on every
/// launch: it always overwrites, so a moved exe self-heals automatically.
pub async fn register_all(exe_path: &Path, base_path: &Path) -> Vec<RegistrationOutcome> {
    let mut results = Vec::new();
    for target in BROWSERS {
        results.push(register_one(target, exe_path, base_path).await);
    }
    results
}

/// Register just one browser by id (Settings' per-browser "Repair"
/// button). `None` if `browser_id` isn't one of [`BROWSERS`].
pub async fn register_one_by_id(browser_id: &str, exe_path: &Path, base_path: &Path) -> Option<RegistrationOutcome> {
    let target = BROWSERS.iter().find(|b| b.id == browser_id)?;
    Some(register_one(target, exe_path, base_path).await)
}

/// Remove just one browser's manifest by id (Settings' per-browser
/// "Remove" button). A no-op (not an error) if `browser_id` is unknown.
pub async fn remove_one(browser_id: &str, base_path: &Path) {
    let Some(target) = BROWSERS.iter().find(|b| b.id == browser_id) else {
        return;
    };
    remove_one_target(target, base_path).await;
}

#[allow(unused_variables)]
async fn register_one(target: &BrowserTarget, exe_path: &Path, base_path: &Path) -> RegistrationOutcome {
    #[cfg(target_os = "linux")]
    {
        let Some(home) = dirs::home_dir() else {
            return RegistrationOutcome {
                browser_id: target.id,
                registered: false,
                detail: "no home directory".to_string(),
            };
        };
        let Some(path) = linux_manifest_path(&home, target) else {
            return RegistrationOutcome {
                browser_id: target.id,
                registered: false,
                detail: "not applicable on this platform".to_string(),
            };
        };
        let content = manifest_json(target, exe_path);
        return match write_manifest_file(&path, &content).await {
            Ok(()) => RegistrationOutcome {
                browser_id: target.id,
                registered: true,
                detail: path.display().to_string(),
            },
            Err(e) => RegistrationOutcome {
                browser_id: target.id,
                registered: false,
                detail: e.to_string(),
            },
        };
    }

    #[cfg(target_os = "windows")]
    {
        let path = windows_manifest_path(base_path, target);
        let content = manifest_json(target, exe_path);
        if let Err(e) = write_manifest_file(&path, &content).await {
            return RegistrationOutcome {
                browser_id: target.id,
                registered: false,
                detail: e.to_string(),
            };
        }
        let Some(hive) = target.registry_hive else {
            // No own registry key (reads another vendor's), but the
            // manifest file is still written above so that vendor's key
            // (registered separately in this same loop) covers it.
            return RegistrationOutcome {
                browser_id: target.id,
                registered: true,
                detail: format!("{} (shares another browser's registry key)", path.display()),
            };
        };
        return match windows_register_key(hive, &path) {
            Ok(()) => RegistrationOutcome {
                browser_id: target.id,
                registered: true,
                detail: path.display().to_string(),
            },
            Err(e) => RegistrationOutcome {
                browser_id: target.id,
                registered: false,
                detail: e.to_string(),
            },
        };
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (target, exe_path, base_path);
        RegistrationOutcome {
            browser_id: target.id,
            registered: false,
            detail: "browser native messaging registration is not implemented on this platform".to_string(),
        }
    }
}

async fn write_manifest_file(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await
}

#[cfg(target_os = "windows")]
fn windows_register_key(hive_subkey: &str, manifest_path: &Path) -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(format!("{hive_subkey}\\{HOST_NAME}"))?;
    key.set_value("", &manifest_path.to_string_lossy().to_string())?;
    Ok(())
}

/// Remove every manifest file this app registered (best-effort; a browser
/// that's never been registered is simply skipped). Windows registry keys
/// are left for the NSIS uninstaller, which removes them directly; this
/// path is for the in-app "Remove" button and portable/Linux cleanup.
pub async fn remove_all(base_path: &Path) {
    for target in BROWSERS {
        remove_one_target(target, base_path).await;
    }
}

#[allow(unused_variables)]
async fn remove_one_target(target: &BrowserTarget, base_path: &Path) {
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = dirs::home_dir() {
            if let Some(path) = linux_manifest_path(&home, target) {
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let path = windows_manifest_path(base_path, target);
        let _ = tokio::fs::remove_file(&path).await;
        if let Some(hive) = target.registry_hive {
            windows_remove_key(hive);
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (target, base_path);
    }
}

#[cfg(target_os = "windows")]
fn windows_remove_key(hive_subkey: &str) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey(hive_subkey) {
        let _ = key.delete_subkey(HOST_NAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_manifest_has_expected_shape() {
        let json = chromium_manifest_json(Path::new("/opt/ddmm/ddmm"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], HOST_NAME);
        assert_eq!(value["path"], "/opt/ddmm/ddmm");
        assert_eq!(value["type"], "stdio");
        assert_eq!(
            value["allowed_origins"][0],
            "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/"
        );
    }

    #[test]
    fn firefox_manifest_has_expected_shape() {
        let json = firefox_manifest_json(Path::new("/opt/ddmm/ddmm"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], HOST_NAME);
        assert_eq!(value["allowed_extensions"][0], "ddmm@katsyk.github.io");
        assert!(value.get("allowed_origins").is_none());
    }

    #[test]
    fn manifest_escapes_windows_backslashes_and_quotes() {
        let json = chromium_manifest_json(Path::new(r#"C:\Program Files\DDMM\ddmm.exe"#));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["path"], r#"C:\Program Files\DDMM\ddmm.exe"#);
    }

    #[test]
    fn linux_manifest_paths_are_distinct_per_browser() {
        let home = Path::new("/home/user");
        let mut paths = std::collections::HashSet::new();
        for target in BROWSERS {
            if let Some(path) = linux_manifest_path(home, target) {
                assert!(paths.insert(path), "duplicate manifest path for {}", target.id);
            }
        }
        // Firefox + five Chromium-family browsers.
        assert_eq!(paths.len(), 6);
    }

    #[test]
    fn linux_firefox_path_is_host_agnostic_of_profile() {
        let home = Path::new("/home/user");
        let firefox = BROWSERS.iter().find(|b| b.id == "firefox").unwrap();
        let path = linux_manifest_path(home, firefox).unwrap();
        assert_eq!(path, home.join(".mozilla/native-messaging-hosts/io.github.katsyk.ddmm.json"));
    }

    #[test]
    fn linux_chrome_path_matches_documented_location() {
        let home = Path::new("/home/user");
        let chrome = BROWSERS.iter().find(|b| b.id == "chrome").unwrap();
        let path = linux_manifest_path(home, chrome).unwrap();
        assert_eq!(
            path,
            home.join(".config/google-chrome/NativeMessagingHosts/io.github.katsyk.ddmm.json")
        );
    }

    #[test]
    fn windows_manifest_paths_are_distinct_per_browser() {
        let base = Path::new("/data");
        let mut paths = std::collections::HashSet::new();
        for target in BROWSERS {
            let path = windows_manifest_path(base, target);
            assert!(paths.insert(path), "duplicate manifest path for {}", target.id);
        }
        assert_eq!(paths.len(), BROWSERS.len());
    }

    #[test]
    fn every_browser_has_a_stable_id() {
        let ids: std::collections::HashSet<_> = BROWSERS.iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), BROWSERS.len());
    }

    #[tokio::test]
    async fn write_manifest_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("manifest.json");
        write_manifest_file(&path, "{}").await.unwrap();
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "{}");
    }
}
