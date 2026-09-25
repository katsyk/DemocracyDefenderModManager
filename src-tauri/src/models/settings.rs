use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod ascii_string {
    use serde::{Deserializer, Serializer};

    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[[u8; 16]], serializer: S) -> Result<S::Ok, S::Error> {
        let strings: Vec<&str> = bytes.iter()
            .map(|arr| str::from_utf8(arr).map_err(serde::ser::Error::custom))
            .collect::<Result<_, _>>()?;
        strings.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<[u8; 16]>, D::Error> {
        let strings = Vec::<String>::deserialize(deserializer)?;
        let mut result = Vec::with_capacity(strings.len());
        for s in strings {
            let bytes = s.as_bytes();
            if bytes.len() != 16 {
                return Err(serde::de::Error::custom("expected 16 characters"));
            }
            let mut arr = [0u8; 16];
            arr.copy_from_slice(bytes);
            result.push(arr);
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "Version", rename_all = "PascalCase")]
pub enum Settings {
    #[serde(rename_all = "PascalCase")]
    V1 {
        game_path: PathBuf,
        #[serde(with = "ascii_string")]
        skip_list: Vec<[u8; 16]>,
        /// Where the browser handoff (see `commands::handoff`) watches for a
        /// freshly-downloaded archive. Defaults to the OS Downloads
        /// directory; `#[serde(default)]` so settings.json files written
        /// before this field existed still load, and `do_load_settings`
        /// fills in the OS default if it comes back empty.
        #[serde(default)]
        downloads_path: PathBuf,
        /// What a one-click browser install does after the mod lands:
        /// `"library"`, `"profile"`, or `"deploy"` (the default -- so a
        /// browser install really is one click end to end).
        #[serde(default = "default_after_browser_install")]
        after_browser_install: String,
        /// Registrable domains (see `bridge::allowlist::registrable_domain`)
        /// the user chose "Always allow" for when the bridge asked to
        /// install from them. Revocable in Settings.
        #[serde(default)]
        bridge_allowed_sites: Vec<String>,
        /// Opt-in: watch `downloads_path` while DDMM is running and offer
        /// to install any new archive that looks like a Helldivers 2 mod.
        /// Off by default -- never installs without a click either way.
        #[serde(default)]
        auto_import_enabled: bool,
    }
}

fn default_after_browser_install() -> String {
    "deploy".to_string()
}

impl Settings {
    pub async fn validate(&self) -> anyhow::Result<()> {
        match self {
            Settings::V1 { game_path, .. } => {
                if game_path.as_os_str().is_empty() {
                    anyhow::bail!("`game_path` is empty");
                }
                
                if !tokio::fs::try_exists(game_path).await.unwrap_or(false) {
                    anyhow::bail!("`game_path` doesn't exist");
                } else {
                    if !tokio::fs::try_exists(game_path.join("tools")).await.unwrap_or(false) {
                        anyhow::bail!("`game_path` doesn't contain dir \"tools\"");
                    }
                    if !tokio::fs::try_exists(game_path.join("data")).await.unwrap_or(false) {
                        anyhow::bail!("`game_path` doesn't contain dir \"data\"");
                    }
                    let bin_path = game_path.join("bin");
                    if !tokio::fs::try_exists(&bin_path).await.unwrap_or(false) {
                        anyhow::bail!("`game_path` doesn't contain dir \"bin\"");
                    } else {
                        if !tokio::fs::try_exists(bin_path.join("helldivers2.exe")).await.unwrap_or(false) {
                            anyhow::bail!("\"bin\" dir does not contain \"helldivers2.exe\"");
                        }
                    }
                }
                
                Ok(())
            },
        }
    }

    pub fn game_path(&self) -> &Path {
        match self {
            Settings::V1 { game_path, .. } => {
                game_path.as_path()
            },
        }
    }

    pub fn set_game_path(&mut self, path: PathBuf) {
        match self {
            Settings::V1 { game_path, .. } => *game_path = path,
        }
    }

    pub fn downloads_path(&self) -> &Path {
        match self {
            Settings::V1 { downloads_path, .. } => downloads_path.as_path(),
        }
    }

    pub fn set_downloads_path(&mut self, path: PathBuf) {
        match self {
            Settings::V1 { downloads_path, .. } => *downloads_path = path,
        }
    }

    pub fn after_browser_install(&self) -> &str {
        match self {
            Settings::V1 { after_browser_install, .. } => after_browser_install.as_str(),
        }
    }

    pub fn set_after_browser_install(&mut self, value: String) {
        match self {
            Settings::V1 { after_browser_install, .. } => *after_browser_install = value,
        }
    }

    pub fn bridge_allowed_sites(&self) -> &[String] {
        match self {
            Settings::V1 { bridge_allowed_sites, .. } => bridge_allowed_sites.as_slice(),
        }
    }

    pub fn is_bridge_site_allowed(&self, registrable_domain: &str) -> bool {
        self.bridge_allowed_sites().iter().any(|s| s == registrable_domain)
    }

    pub fn allow_bridge_site(&mut self, registrable_domain: String) {
        match self {
            Settings::V1 { bridge_allowed_sites, .. } => {
                if !bridge_allowed_sites.contains(&registrable_domain) {
                    bridge_allowed_sites.push(registrable_domain);
                }
            }
        }
    }

    pub fn revoke_bridge_site(&mut self, registrable_domain: &str) {
        match self {
            Settings::V1 { bridge_allowed_sites, .. } => {
                bridge_allowed_sites.retain(|s| s != registrable_domain);
            }
        }
    }

    pub fn auto_import_enabled(&self) -> bool {
        match self {
            Settings::V1 { auto_import_enabled, .. } => *auto_import_enabled,
        }
    }

    pub fn set_auto_import_enabled(&mut self, value: bool) {
        match self {
            Settings::V1 { auto_import_enabled, .. } => *auto_import_enabled = value,
        }
    }

    pub fn has_skip_entry(&self, s: &str) -> bool {
        match self {
            Settings::V1 { skip_list, .. } => {
                skip_list.iter()
                    .filter_map(|entry| {
                        str::from_utf8(entry).ok()
                    })
                    .any(|entry| entry == s)
            },
        }
    }
}