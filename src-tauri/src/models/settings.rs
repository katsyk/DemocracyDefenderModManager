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
        /// Opt-in: check installed mods for updates when DDMM starts. Off
        /// by default -- update checks never run unless the user asks
        /// (this, or clicking "Check for Updates").
        #[serde(default)]
        auto_check_updates: bool,
        /// With `auto_check_updates` on: also re-check every this many
        /// hours while DDMM stays open. `0` (the default) means only at
        /// startup.
        #[serde(default)]
        auto_check_interval_hours: u32,
        /// Display name of the Nexus Mods account whose (optional) API key
        /// is stored -- shown in Settings. The key itself is never stored
        /// here (see `crate::secrets`). Owned by the backend: `save_settings`
        /// keeps the value already on disk.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nexus_username: Option<String>,
    }
}

/// Bounds for [`Settings::auto_check_interval_hours`]: at most once an hour,
/// at least once a week.
pub const MIN_AUTO_CHECK_INTERVAL_HOURS: u32 = 1;
pub const MAX_AUTO_CHECK_INTERVAL_HOURS: u32 = 168;

fn default_after_browser_install() -> String {
    "deploy".to_string()
}

impl Settings {
    /// Check the configured game path (see [`crate::game_path::resolve`])
    /// and return the actual game root to use -- which may differ from the
    /// stored path if the user picked e.g. its `data/` folder.
    pub async fn validate(&self) -> anyhow::Result<PathBuf> {
        match self {
            Settings::V1 { game_path, .. } => crate::game_path::resolve(game_path)
                .await
                .map_err(|problem| anyhow::anyhow!(problem.describe(game_path))),
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

    pub fn auto_check_updates(&self) -> bool {
        match self {
            Settings::V1 { auto_check_updates, .. } => *auto_check_updates,
        }
    }

    /// The re-check interval, if one is set (clamped to the allowed range).
    pub fn auto_check_interval_hours(&self) -> Option<u32> {
        match self {
            Settings::V1 { auto_check_interval_hours: 0, .. } => None,
            Settings::V1 { auto_check_interval_hours, .. } => Some(
                (*auto_check_interval_hours).clamp(MIN_AUTO_CHECK_INTERVAL_HOURS, MAX_AUTO_CHECK_INTERVAL_HOURS),
            ),
        }
    }

    pub fn nexus_username(&self) -> Option<&str> {
        match self {
            Settings::V1 { nexus_username, .. } => nexus_username.as_deref(),
        }
    }

    pub fn set_nexus_username(&mut self, value: Option<String>) {
        match self {
            Settings::V1 { nexus_username, .. } => *nexus_username = value,
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
#[cfg(test)]
mod update_setting_tests {
    use super::*;

    #[test]
    fn automatic_update_checks_are_off_by_default() {
        // A settings.json from before these options existed.
        let old = r#"{"Version":"V1","GamePath":"","SkipList":[]}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(!s.auto_check_updates());
        assert_eq!(s.auto_check_interval_hours(), None);
        assert_eq!(s.nexus_username(), None);
    }

    #[test]
    fn interval_is_clamped() {
        let s: Settings = serde_json::from_str(
            r#"{"Version":"V1","GamePath":"","SkipList":[],"AutoCheckUpdates":true,"AutoCheckIntervalHours":100000}"#,
        )
        .unwrap();
        assert!(s.auto_check_updates());
        assert_eq!(s.auto_check_interval_hours(), Some(MAX_AUTO_CHECK_INTERVAL_HOURS));
    }

    #[test]
    fn settings_json_never_has_a_key_field() {
        let mut s: Settings = serde_json::from_str(r#"{"Version":"V1","GamePath":"","SkipList":[]}"#).unwrap();
        s.set_nexus_username(Some("Diver".into()));
        let json = serde_json::to_string(&s).unwrap().to_ascii_lowercase();
        assert!(json.contains("nexususername"));
        assert!(!json.contains("apikey") && !json.contains("api_key"), "{json}");
    }
}
