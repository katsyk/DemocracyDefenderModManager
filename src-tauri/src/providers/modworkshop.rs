//! ModWorkshop: the public API at `https://api.modworkshop.net`, keyless
//! (it reports `x-ratelimit-limit: 90` per minute).
//!
//! - `GET /mods/{id}`: `version`, `bumped_at` (last real update --
//!   `updated_at` also moves for unrelated edits), and the primary
//!   `download` file.
//! - `GET /mods/{id}/files`: every file, fetched only when an update is
//!   available and the mod has more than one file.
//! - Files download from `https://api.modworkshop.net/files/{id}/download`
//!   (redirects to `storage.modworkshop.net`), publicly. A mod whose
//!   download is an external *link* has no direct file and goes through the
//!   browser instead.

use serde::Deserialize;

use super::{fetch_capped, format_timestamp, has_archive_extension, parse_iso_utc, UpdateFile};

pub const HOST: &str = "api.modworkshop.net";

#[derive(Debug, Clone, Deserialize)]
pub struct MwsMod {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub bumped_at: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub suspended: bool,
    #[serde(default)]
    pub download_type: Option<String>,
    #[serde(default)]
    pub files_count: Option<u64>,
    #[serde(default)]
    pub download: Option<MwsFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MwsFile {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default, rename = "type")]
    pub file_type: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// Present on files; absent on the link shape of `download`.
    #[serde(default)]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MwsFiles {
    #[serde(default)]
    pub data: Vec<MwsFile>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 12 && id.chars().all(|c| c.is_ascii_digit())
}

pub async fn fetch_mod(client: &reqwest::Client, id: &str) -> anyhow::Result<MwsMod> {
    if !valid_id(id) {
        anyhow::bail!("\"{id}\" isn't a ModWorkshop mod id");
    }
    let (body, _) = fetch_capped(client, &format!("https://{HOST}/mods/{id}")).await?;
    parse_mod(&body)
}

pub async fn fetch_files(client: &reqwest::Client, id: &str) -> anyhow::Result<Vec<MwsFile>> {
    if !valid_id(id) {
        anyhow::bail!("\"{id}\" isn't a ModWorkshop mod id");
    }
    let (body, _) = fetch_capped(client, &format!("https://{HOST}/mods/{id}/files")).await?;
    Ok(parse_files(&body)?.data)
}

pub fn parse_mod(body: &str) -> anyhow::Result<MwsMod> {
    Ok(serde_json::from_str(body)?)
}

pub fn parse_files(body: &str) -> anyhow::Result<MwsFiles> {
    Ok(serde_json::from_str(body)?)
}

pub fn is_available(m: &MwsMod) -> bool {
    !m.suspended && m.visibility.as_deref().is_none_or(|v| v == "public" || v == "unlisted")
}

/// `version` when the author set one, otherwise when the mod was last
/// bumped (a real update).
pub fn latest_version(m: &MwsMod) -> Option<String> {
    if let Some(v) = m.version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        return Some(v.to_string());
    }
    m.bumped_at.as_deref().and_then(parse_iso_utc).map(format_timestamp)
}

/// Whether the mod's download is a hosted file (as opposed to an external
/// link, which needs the browser).
pub fn has_direct_files(m: &MwsMod) -> bool {
    m.download_type.as_deref() != Some("link") && m.download.as_ref().is_some_and(|d| d.download_url.is_some() || d.file.is_some())
}

fn to_candidate(f: &MwsFile) -> Option<UpdateFile> {
    let name = f.file.clone().or_else(|| f.name.clone())?;
    let type_ok = f.file_type.as_deref().map(|t| ["zip", "7z", "rar"].contains(&t)).unwrap_or(false);
    if !type_ok && !has_archive_extension(&name) {
        return None;
    }
    let label = f
        .label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .or_else(|| f.name.clone().filter(|n| !n.trim().is_empty()));
    Some(UpdateFile {
        id: f.id.to_string(),
        name,
        label,
        size: f.size,
        uploaded_at: f.created_at.as_deref().and_then(parse_iso_utc),
        // Canonical download endpoint (counts the download for the author,
        // like the site's own button) -- not the storage URL.
        url: format!("https://{HOST}/files/{}/download", f.id),
    })
}

/// One-click candidates from the mod's primary download, or from the full
/// file list when one was fetched.
pub fn candidates(m: &MwsMod, all_files: Option<&[MwsFile]>) -> Vec<UpdateFile> {
    if !has_direct_files(m) {
        return Vec::new();
    }
    match all_files {
        Some(files) if !files.is_empty() => files.iter().filter_map(to_candidate).collect(),
        _ => m.download.iter().filter_map(to_candidate).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOD: &str = include_str!("../../tests/fixtures/updates/modworkshop_mod.json");
    const FILES: &str = include_str!("../../tests/fixtures/updates/modworkshop_files.json");

    #[test]
    fn parses_recorded_mod() {
        let m = parse_mod(MOD).unwrap();
        assert!(is_available(&m));
        assert_eq!(latest_version(&m).as_deref(), Some("4.3"));
        assert!(has_direct_files(&m));
        let c = candidates(&m, None);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].url, "https://api.modworkshop.net/files/96181/download");
        assert!(super::super::is_allowed_download_host("modworkshop", &c[0].url));
    }

    #[test]
    fn parses_recorded_file_list() {
        let m = parse_mod(MOD).unwrap();
        let files = parse_files(FILES).unwrap().data;
        let c = candidates(&m, Some(&files));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].label.as_deref(), Some("Mr Pizza Portable Hellbomb"));
    }

    #[test]
    fn version_falls_back_to_bumped_at() {
        let m = parse_mod(r#"{"version":"","bumped_at":"2026-04-30T04:30:31.000000Z"}"#).unwrap();
        assert_eq!(latest_version(&m).as_deref(), Some("2026-04-30 04:30 UTC"));
    }

    #[test]
    fn external_link_downloads_have_no_direct_files() {
        let m = parse_mod(r#"{"version":"1","download_type":"link","download":{"id":5,"url":"https://example.com"}}"#).unwrap();
        assert!(!has_direct_files(&m));
        assert!(candidates(&m, None).is_empty());
    }

    #[test]
    fn suspended_or_private_mods_are_unavailable() {
        assert!(!is_available(&parse_mod(r#"{"suspended":true}"#).unwrap()));
        assert!(!is_available(&parse_mod(r#"{"visibility":"private"}"#).unwrap()));
    }

    #[tokio::test]
    #[ignore]
    async fn live_mod() {
        let client = super::super::build_client().unwrap();
        let m = fetch_mod(&client, "51110").await.unwrap();
        assert!(latest_version(&m).is_some());
    }
}
