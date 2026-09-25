//! GameBanana: the public apiv11 endpoint for a single mod,
//! `GET https://gamebanana.com/apiv11/Mod/{id}?_csvProperties=...`, keyless.
//! Asking only for the properties we use keeps the response small.
//!
//! Every file has a public direct link (`https://gamebanana.com/dl/{fileId}`,
//! which redirects to `files.gamebanana.com`), so updates can be one click.
//! Many mods don't set `_sVersion`; then the newest file's upload time is
//! the "version" (see [`latest_version`]).

use serde::Deserialize;

use super::{fetch_capped, format_timestamp, has_archive_extension, UpdateFile};

pub const HOST: &str = "gamebanana.com";

const PROPERTIES: &str =
    "_idRow,_sName,_sVersion,_tsDateModified,_tsDateUpdated,_aFiles,_bIsPrivate,_bIsTrashed,_bIsWithheld,_bIsObsolete";

#[derive(Debug, Clone, Deserialize)]
pub struct GbMod {
    #[serde(rename = "_sName", default)]
    pub name: Option<String>,
    #[serde(rename = "_sVersion", default)]
    pub version: Option<String>,
    #[serde(rename = "_tsDateUpdated", default)]
    pub date_updated: Option<i64>,
    #[serde(rename = "_bIsPrivate", default)]
    pub is_private: bool,
    #[serde(rename = "_bIsTrashed", default)]
    pub is_trashed: bool,
    #[serde(rename = "_bIsWithheld", default)]
    pub is_withheld: bool,
    #[serde(rename = "_aFiles", default)]
    pub files: Vec<GbFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GbFile {
    #[serde(rename = "_idRow")]
    pub id: u64,
    #[serde(rename = "_sFile")]
    pub file: String,
    #[serde(rename = "_nFilesize", default)]
    pub size: Option<u64>,
    #[serde(rename = "_tsDateAdded", default)]
    pub date_added: Option<i64>,
    #[serde(rename = "_sDownloadUrl", default)]
    pub download_url: Option<String>,
    #[serde(rename = "_sVersion", default)]
    pub version: Option<String>,
    #[serde(rename = "_sDescription", default)]
    pub description: Option<String>,
    #[serde(rename = "_bIsArchived", default)]
    pub is_archived: bool,
    #[serde(rename = "_sAvResult", default)]
    pub av_result: Option<String>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 12 && id.chars().all(|c| c.is_ascii_digit())
}

pub async fn fetch_mod(client: &reqwest::Client, id: &str) -> anyhow::Result<GbMod> {
    if !valid_id(id) {
        anyhow::bail!("\"{id}\" isn't a GameBanana mod id");
    }
    let url = format!("https://{HOST}/apiv11/Mod/{id}?_csvProperties={PROPERTIES}");
    let (body, _) = fetch_capped(client, &url).await?;
    parse_mod(&body)
}

pub fn parse_mod(body: &str) -> anyhow::Result<GbMod> {
    Ok(serde_json::from_str(body)?)
}

/// Files a one-click update may offer: live (not archived), archive-typed,
/// on GameBanana's own download host, and not flagged by its virus scan.
pub fn live_files(m: &GbMod) -> impl Iterator<Item = &GbFile> {
    m.files.iter().filter(|f| {
        !f.is_archived
            && has_archive_extension(&f.file)
            && !matches!(f.av_result.as_deref(), Some(r) if !r.is_empty() && r != "clean")
    })
}

/// The mod's current version: its `_sVersion` if the author set one,
/// otherwise the newest live file's upload time.
pub fn latest_version(m: &GbMod) -> Option<String> {
    if let Some(v) = m.version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        return Some(v.to_string());
    }
    live_files(m)
        .filter_map(|f| f.date_added)
        .max()
        .or(m.date_updated)
        .map(format_timestamp)
}

/// `None` when the mod is gone/private (then it's not checkable).
pub fn is_available(m: &GbMod) -> bool {
    !(m.is_private || m.is_trashed || m.is_withheld)
}

pub fn candidates(m: &GbMod) -> Vec<UpdateFile> {
    live_files(m)
        .map(|f| UpdateFile {
            id: f.id.to_string(),
            name: f.file.clone(),
            label: f.description.clone().filter(|d| !d.trim().is_empty()),
            size: f.size,
            uploaded_at: f.date_added,
            // Always the canonical dl link (not whatever the payload says)
            // so the host allowlist holds.
            url: format!("https://{HOST}/dl/{}", f.id),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_VERSION: &str = include_str!("../../tests/fixtures/updates/gamebanana_mod.json");
    const NO_VERSION: &str = include_str!("../../tests/fixtures/updates/gamebanana_mod_noversion.json");

    #[test]
    fn parses_recorded_mod_with_version() {
        let m = parse_mod(WITH_VERSION).unwrap();
        assert!(is_available(&m));
        assert_eq!(latest_version(&m).as_deref(), Some("1.3.0"));
        let files = candidates(&m);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].url, "https://gamebanana.com/dl/1688396");
        assert_eq!(files[0].label.as_deref(), Some("SA-8 NO HELM"));
        assert!(files.iter().all(|f| super::super::is_allowed_download_host("gamebanana", &f.url)));
    }

    #[test]
    fn version_falls_back_to_newest_file_upload_time() {
        let m = parse_mod(NO_VERSION).unwrap();
        assert_eq!(m.version.as_deref(), Some(""));
        let v = latest_version(&m).unwrap();
        assert!(v.ends_with(" UTC"), "{v}");
    }

    #[test]
    fn archived_and_flagged_files_are_not_offered() {
        let body = r#"{"_sVersion":"2","_aFiles":[
            {"_idRow":1,"_sFile":"a.zip","_bIsArchived":true,"_sAvResult":"clean"},
            {"_idRow":2,"_sFile":"b.zip","_bIsArchived":false,"_sAvResult":"infected"},
            {"_idRow":3,"_sFile":"c.exe","_bIsArchived":false,"_sAvResult":"clean"},
            {"_idRow":4,"_sFile":"d.7z","_bIsArchived":false,"_sAvResult":"clean"}]}"#;
        let m = parse_mod(body).unwrap();
        let ids: Vec<_> = candidates(&m).into_iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["4"]);
    }

    #[test]
    fn ids_are_numeric_only() {
        assert!(valid_id("669221"));
        assert!(!valid_id("66a"));
        assert!(!valid_id("1/../2"));
    }

    #[tokio::test]
    #[ignore]
    async fn live_mod() {
        let client = super::super::build_client().unwrap();
        let m = fetch_mod(&client, "669221").await.unwrap();
        assert!(latest_version(&m).is_some());
    }
}
