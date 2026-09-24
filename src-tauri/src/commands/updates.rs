//! Update checks -- only ever run when the user explicitly asks (there is
//! no background polling, no check at startup). Supports the providers
//! that expose a way to check without an API key: AyakaMods (scrapes the
//! JSON-LD `SoftwareApplication` block already on every mod page) and
//! GitHub (the public releases API). Nexus Mods requires a personal API
//! key this manager doesn't ask for, so it reports "unsupported" rather
//! than guessing or erroring.

use std::{collections::HashMap, time::Duration};

use anyhow_tauri::{IntoTAResult, TAResult};
use futures::StreamExt;
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::{sources, sources::ResolvedSource, AppState};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PAGE_SIZE: u64 = 5 * 1024 * 1024;
const SAME_HOST_DELAY: Duration = Duration::from_secs(1);

/// Providers this manager knows how to check for updates. Anything else is
/// skipped entirely (no entry produced) rather than reported as
/// unsupported -- "unsupported" is reserved for providers users would
/// reasonably expect update checking from (Nexus) but that need
/// credentials this manager doesn't collect.
const CHECKABLE_PROVIDERS: [&str; 3] = ["ayakamods", "github", "nexus"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateStatusEntry {
    pub guid: Uuid,
    pub provider: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    pub status: UpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "Kind", rename_all = "PascalCase")]
pub enum UpdateState {
    UpToDate,
    UpdateAvailable,
    /// Not enough information to say either way (no installed version on
    /// record, or the remote check came back empty).
    Unknown,
    /// A known provider (Nexus) that this manager can't check without
    /// credentials it doesn't ask for.
    Unsupported,
    Error { message: String },
}

/// Check every installed mod's declared/recorded sources against each
/// provider's latest published version. Only ever invoked by the user
/// clicking "Check for updates" -- never on a timer, never at startup.
#[tauri::command]
pub async fn check_updates(state: State<'_, AppState>) -> TAResult<Vec<UpdateStatusEntry>> {
    let mods = {
        let guard = state.mods.lock().await;
        guard.clone().unwrap_or_default()
    };

    let client = build_client().into_ta_result()?;
    let mut last_request: HashMap<&'static str, tokio::time::Instant> = HashMap::new();
    let mut results = Vec::new();

    for m in &mods {
        for source in &m.sources {
            if !CHECKABLE_PROVIDERS.contains(&source.provider.as_str()) {
                continue;
            }

            if let Some(host) = provider_host(&source.provider) {
                rate_limit(&mut last_request, host).await;
            }

            let (status, latest) = check_source(&client, source).await;

            results.push(UpdateStatusEntry {
                guid: m.guid(),
                provider: source.provider.clone(),
                display_name: source.display_name.clone(),
                installed_version: source.version.clone(),
                latest_version: latest,
                status,
                page_url: source.page_url.clone(),
            });
        }
    }

    Ok(results)
}

async fn rate_limit(last_request: &mut HashMap<&'static str, tokio::time::Instant>, host: &'static str) {
    if let Some(last) = last_request.get(host) {
        let elapsed = last.elapsed();
        if elapsed < SAME_HOST_DELAY {
            tokio::time::sleep(SAME_HOST_DELAY - elapsed).await;
        }
    }
    last_request.insert(host, tokio::time::Instant::now());
}

fn provider_host(provider: &str) -> Option<&'static str> {
    match provider {
        "ayakamods" => Some("ayakamods.com"),
        "github" => Some("api.github.com"),
        _ => None,
    }
}

async fn check_source(client: &reqwest::Client, source: &ResolvedSource) -> (UpdateState, Option<String>) {
    match source.provider.as_str() {
        "nexus" => (UpdateState::Unsupported, None),
        "ayakamods" => {
            let Some(id) = id_from_page_url(source) else {
                return (UpdateState::Unknown, None);
            };
            match fetch_ayakamods_metadata(client, &id).await {
                Ok(Some(meta)) => {
                    let status = compute_status(source.version.as_deref(), meta.latest_version.as_deref());
                    (status, meta.latest_version)
                }
                Ok(None) => (UpdateState::Unknown, None),
                Err(e) => (UpdateState::Error { message: e.to_string() }, None),
            }
        }
        "github" => {
            let Some(id) = id_from_page_url(source) else {
                return (UpdateState::Unknown, None);
            };
            match fetch_github_latest_tag(client, &id).await {
                Ok(Some(tag)) => {
                    let status = compute_status(source.version.as_deref(), Some(&tag));
                    (status, Some(tag))
                }
                Ok(None) => (UpdateState::Unknown, None),
                Err(e) => (UpdateState::Error { message: e.to_string() }, None),
            }
        }
        _ => (UpdateState::Unsupported, None),
    }
}

/// Recover the raw provider id (ayakamods numeric id, or `owner/repo` for
/// GitHub) from a resolved source's page URL by re-parsing it the same way
/// a pasted page URL would be. `ResolvedSource` doesn't carry the raw id
/// itself, so this is how update checks get back to it without needing to
/// widen that (frontend-facing) type.
fn id_from_page_url(source: &ResolvedSource) -> Option<String> {
    let page_url = source.page_url.as_ref()?;
    sources::source_from_page_url(page_url)?.id
}

fn compute_status(installed: Option<&str>, latest: Option<&str>) -> UpdateState {
    let (Some(installed), Some(latest)) = (installed, latest) else {
        return UpdateState::Unknown;
    };
    if latest.is_empty() {
        return UpdateState::Unknown;
    }
    if installed == latest {
        UpdateState::UpToDate
    } else {
        UpdateState::UpdateAvailable
    }
}

pub(crate) fn build_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("ddmm/{}", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()?)
}

/// GET `url` (https only), capped at [`MAX_PAGE_SIZE`] bytes (checked
/// against both `Content-Length` and the actual streamed size, the same
/// belt-and-braces approach `download::download_archive` uses), returned
/// as text.
async fn fetch_capped(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "https" {
        anyhow::bail!("only https:// URLs are supported");
    }

    let response = client.get(parsed).send().await?.error_for_status()?;

    if let Some(len) = response.content_length() {
        if len > MAX_PAGE_SIZE {
            anyhow::bail!("page is {} bytes, which exceeds the {} byte limit", len, MAX_PAGE_SIZE);
        }
    }

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > MAX_PAGE_SIZE {
            anyhow::bail!("page exceeded the {} byte limit while streaming", MAX_PAGE_SIZE);
        }
    }

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AyakaModsMetadata {
    pub name: Option<String>,
    pub latest_version: Option<String>,
    pub modified: Option<String>,
}

pub(crate) async fn fetch_ayakamods_metadata(
    client: &reqwest::Client,
    id: &str,
) -> anyhow::Result<Option<AyakaModsMetadata>> {
    let url = format!("https://ayakamods.com/mods/{id}/");
    let html = fetch_capped(client, &url).await?;
    Ok(parse_ayakamods_page(&html))
}

/// Extract the mod's `SoftwareApplication` JSON-LD block from a saved (or
/// freshly-fetched) AyakaMods mod page. `None` for anything that doesn't
/// parse -- missing/malformed JSON-LD is not an error, it's just
/// [`UpdateState::Unknown`] to the caller.
fn parse_ayakamods_page(html: &str) -> Option<AyakaModsMetadata> {
    for block in extract_ld_json_blocks(html) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&block) else {
            continue;
        };
        if let Some(app) = find_software_application(&value) {
            return Some(AyakaModsMetadata {
                name: app.get("name").and_then(|v| v.as_str()).map(str::to_string),
                latest_version: app
                    .get("softwareVersion")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                modified: app
                    .get("dateModified")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }
    None
}

/// Scan `html` for `<script type="application/ld+json">...</script>`
/// blocks. Deliberately simple (no HTML parser crate, just `regex` -- an
/// existing dependency): good enough for extracting a well-formed script
/// tag's contents, not a general HTML parser.
fn extract_ld_json_blocks(html: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(?is)<script[^>]*type\s*=\s*"application/ld\+json"[^>]*>(.*?)</script>"#)
            .unwrap()
    });
    re.captures_iter(html)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Depth-first search through a JSON-LD value (a plain object, an array of
/// objects/graphs, or an object with an `@graph` array) for the first node
/// whose `@type` is `SoftwareApplication`.
fn find_software_application(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => items.iter().find_map(find_software_application),
        serde_json::Value::Object(map) => {
            if map.get("@type").and_then(|v| v.as_str()) == Some("SoftwareApplication") {
                return Some(value);
            }
            map.get("@graph").and_then(find_software_application)
        }
        _ => None,
    }
}

async fn fetch_github_latest_tag(client: &reqwest::Client, owner_repo: &str) -> anyhow::Result<Option<String>> {
    let url = format!("https://api.github.com/repos/{owner_repo}/releases/latest");
    let body = fetch_capped(client, &url).await?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    Ok(value.get("tag_name").and_then(|v| v.as_str()).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/ayakamods_mod_page.html");

    #[test]
    fn parses_real_ayakamods_fixture() {
        let meta = parse_ayakamods_page(FIXTURE).expect("should find JSON-LD");
        assert_eq!(meta.name.as_deref(), Some("HD2 Auto Reload"));
        assert_eq!(meta.latest_version.as_deref(), Some("2026-09-24"));
        assert!(meta.modified.is_some());
    }

    #[test]
    fn missing_ld_json_is_none_not_error() {
        let meta = parse_ayakamods_page("<html><body>no json-ld here</body></html>");
        assert!(meta.is_none());
    }

    #[test]
    fn malformed_ld_json_is_none_not_error() {
        let html = r#"<script type="application/ld+json">{ not: valid json </script>"#;
        let meta = parse_ayakamods_page(html);
        assert!(meta.is_none());
    }

    #[test]
    fn ld_json_without_software_application_is_none() {
        let html = r#"<script type="application/ld+json">[{"@type":"Organization","name":"Someone"}]</script>"#;
        let meta = parse_ayakamods_page(html);
        assert!(meta.is_none());
    }

    #[test]
    fn ld_json_graph_form_is_found() {
        let html = r#"<script type="application/ld+json">
            {"@graph": [{"@type":"Organization"}, {"@type":"SoftwareApplication","name":"Graph Mod","softwareVersion":"1.2.3"}]}
        </script>"#;
        let meta = parse_ayakamods_page(html).expect("should find nested graph entry");
        assert_eq!(meta.name.as_deref(), Some("Graph Mod"));
        assert_eq!(meta.latest_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn compute_status_up_to_date() {
        assert_eq!(compute_status(Some("1.0"), Some("1.0")), UpdateState::UpToDate);
    }

    #[test]
    fn compute_status_update_available() {
        assert_eq!(compute_status(Some("1.0"), Some("2.0")), UpdateState::UpdateAvailable);
    }

    #[test]
    fn compute_status_unknown_installed() {
        assert_eq!(compute_status(None, Some("2.0")), UpdateState::Unknown);
    }

    #[test]
    fn compute_status_unknown_latest() {
        assert_eq!(compute_status(Some("1.0"), None), UpdateState::Unknown);
    }

    #[test]
    fn compute_status_unknown_when_latest_empty() {
        assert_eq!(compute_status(Some("1.0"), Some("")), UpdateState::Unknown);
    }

    #[test]
    fn id_from_page_url_ayakamods() {
        let source = ResolvedSource {
            provider: "ayakamods".to_string(),
            display_name: "AyakaMods".to_string(),
            page_url: Some("https://ayakamods.com/mods/4084/".to_string()),
            version: None,
            origin: sources::SourceOrigin::Manifest,
        };
        assert_eq!(id_from_page_url(&source).as_deref(), Some("4084"));
    }

    #[test]
    fn id_from_page_url_github() {
        let source = ResolvedSource {
            provider: "github".to_string(),
            display_name: "GitHub".to_string(),
            page_url: Some("https://github.com/someone/example-mod".to_string()),
            version: None,
            origin: sources::SourceOrigin::Manifest,
        };
        assert_eq!(id_from_page_url(&source).as_deref(), Some("someone/example-mod"));
    }

    #[test]
    fn id_from_page_url_none_when_no_page_url() {
        let source = ResolvedSource {
            provider: "ayakamods".to_string(),
            display_name: "AyakaMods".to_string(),
            page_url: None,
            version: None,
            origin: sources::SourceOrigin::Manifest,
        };
        assert!(id_from_page_url(&source).is_none());
    }

    /// Real network smoke test against the live page. `#[ignore]`d so the
    /// normal suite never depends on network access.
    #[tokio::test]
    #[ignore]
    async fn fetches_and_parses_the_real_page() {
        let client = build_client().unwrap();
        let meta = fetch_ayakamods_metadata(&client, "hd2-auto-reload.4084")
            .await
            .unwrap()
            .expect("should parse JSON-LD from the live page");
        assert!(meta.latest_version.is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn fetches_the_real_github_latest_release_tag() {
        let client = build_client().unwrap();
        let tag = fetch_github_latest_tag(&client, "tauri-apps/tauri")
            .await
            .unwrap();
        assert!(tag.is_some());
    }
}
