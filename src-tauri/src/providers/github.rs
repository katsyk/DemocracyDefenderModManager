//! GitHub: the public REST API's "latest release" endpoint
//! (`GET https://api.github.com/repos/{owner}/{repo}/releases/latest`),
//! keyless (60 requests/hour per IP). The release's zip/7z/rar assets are
//! public direct downloads, so a GitHub update can be one click.

use serde::Deserialize;

use super::{fetch_capped, has_archive_extension, UpdateFile};

pub const API_HOST: &str = "api.github.com";

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub size: Option<u64>,
    pub browser_download_url: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// `owner/repo` must be exactly two plain path segments -- it's spliced into
/// an API URL.
pub fn valid_repo_id(owner_repo: &str) -> bool {
    let mut parts = owner_repo.split('/');
    let ok = |s: Option<&str>| {
        s.is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && s != "." && s != "..")
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

pub async fn latest_release(client: &reqwest::Client, owner_repo: &str) -> anyhow::Result<Option<Release>> {
    if !valid_repo_id(owner_repo) {
        anyhow::bail!("\"{owner_repo}\" isn't a valid GitHub owner/repo");
    }
    let url = format!("https://{API_HOST}/repos/{owner_repo}/releases/latest");
    match fetch_capped(client, &url).await {
        Ok((body, _)) => Ok(Some(parse_release(&body)?)),
        // No published (non-pre-)release yet.
        Err(e) if e.to_string().contains("404") => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn parse_release(body: &str) -> anyhow::Result<Release> {
    Ok(serde_json::from_str(body)?)
}

/// The release's installable assets (zip/7z/rar), as one-click candidates.
pub fn candidates(release: &Release) -> Vec<UpdateFile> {
    release
        .assets
        .iter()
        .filter(|a| has_archive_extension(&a.name))
        .filter(|a| super::is_allowed_download_host("github", &a.browser_download_url))
        .map(|a| UpdateFile {
            id: a.id.to_string(),
            name: a.name.clone(),
            label: None,
            size: a.size,
            uploaded_at: a.updated_at.as_deref().and_then(super::parse_iso_utc),
            url: a.browser_download_url.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/updates/github_latest_release.json");

    #[test]
    fn parses_recorded_release_and_keeps_only_archives() {
        let release = parse_release(FIXTURE).unwrap();
        assert_eq!(release.tag_name.as_deref(), Some("v1.4.0"));
        let files = candidates(&release);
        let names: Vec<_> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["ExampleMod-1.4.0.zip", "ExampleMod-1.4.0-no-sounds.7z"]);
        assert!(files.iter().all(|f| f.url.starts_with("https://github.com/")));
    }

    #[test]
    fn repo_ids_are_validated() {
        assert!(valid_repo_id("owner/repo"));
        assert!(valid_repo_id("my-org/My.Repo_2"));
        assert!(!valid_repo_id("owner"));
        assert!(!valid_repo_id("owner/repo/extra"));
        assert!(!valid_repo_id("../etc"));
        assert!(!valid_repo_id("owner/.."));
        assert!(!valid_repo_id("owner/re po"));
    }

    #[tokio::test]
    #[ignore]
    async fn live_latest_release() {
        let client = super::super::build_client().unwrap();
        let release = latest_release(&client, "tauri-apps/tauri").await.unwrap();
        assert!(release.and_then(|r| r.tag_name).is_some());
    }
}
