//! Provider-neutral mod source resolution.
//!
//! This module turns the free-form [`Source`](crate::models::manifest::Source)
//! declarations found in a manifest (plus the legacy `NexusData` field, plus
//! whatever the manager itself recorded when a mod was installed) into
//! [`ResolvedSource`] values the frontend can render as "Open on <site>" menu
//! entries.
//!
//! No provider is privileged: unknown provider strings are passed straight
//! through, and any explicit `Url` always wins over a well-known template.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::models::manifest::Source;

/// The sidecar file the manager writes next to a mod's `manifest.json` when
/// it records where a mod was installed from. Never merged into, or
/// overwriting, the author's own manifest.
pub const ORIGIN_SIDECAR_FILE: &str = ".hd2mm-origin.json";

/// Where a [`ResolvedSource`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SourceOrigin {
    /// Declared by the mod author in the manifest (`Sources` or legacy `NexusData`).
    Manifest,
    /// Recorded by the manager when the user installed the mod.
    Install,
}

/// A [`Source`] resolved into something directly renderable by the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResolvedSource {
    pub provider: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub origin: SourceOrigin,
}

/// The sidecar written next to a mod directory when it is installed from a
/// URL (or otherwise given a recorded origin the manifest itself doesn't
/// declare).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OriginSidecar {
    #[serde(default)]
    pub sources: Vec<Source>,
    pub installed_at: u64,
}

/// Only ever allow a page URL through if it is a plain `http(s)` URL. This is
/// opened via the OS's URL opener, so anything else (`file:`, `javascript:`,
/// a custom scheme, ...) must never make it through.
fn sanitize_http_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Resolve a single declared [`Source`] into a [`ResolvedSource`].
///
/// The returned origin is always [`SourceOrigin::Manifest`]; callers that
/// resolve install-recorded sources should override it afterwards.
pub fn resolve(source: &Source) -> ResolvedSource {
    let provider_key = source.provider.trim().to_ascii_lowercase();

    let template_url = match provider_key.as_str() {
        "nexus" => source
            .id
            .as_ref()
            .map(|id| format!("https://www.nexusmods.com/helldivers2/mods/{id}")),
        "modworkshop" => source
            .id
            .as_ref()
            .map(|id| format!("https://modworkshop.net/mod/{id}")),
        "github" => source
            .id
            .as_ref()
            .map(|id| format!("https://github.com/{id}")),
        "gamebanana" => source
            .id
            .as_ref()
            .map(|id| format!("https://gamebanana.com/mods/{id}")),
        "ayakamods" => source
            .id
            .as_ref()
            .map(|id| format!("https://ayakamods.com/mods/{id}/")),
        _ => None,
    };

    let display_name = match provider_key.as_str() {
        "nexus" => "Nexus Mods".to_string(),
        "modworkshop" => "ModWorkshop".to_string(),
        "github" => "GitHub".to_string(),
        "gamebanana" => "GameBanana".to_string(),
        "ayakamods" => "AyakaMods".to_string(),
        "url" => "Link".to_string(),
        _ => source.provider.clone(),
    };

    // An explicit Url always wins over the template, for every provider.
    let candidate = source.url.clone().or(template_url);
    let page_url = candidate.and_then(|u| sanitize_http_url(&u));

    ResolvedSource {
        provider: source.provider.clone(),
        display_name,
        page_url,
        version: source.version.clone(),
        origin: SourceOrigin::Manifest,
    }
}

/// Extract the host portion of a URL without pulling in a full URL-parsing
/// dependency. Best-effort: returns `None` for anything that doesn't look
/// like `scheme://host[:port][/...]`.
fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = host_port.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Guess a provider id from a raw URL's host, for URLs the user pasted in
/// directly rather than a manifest-declared source.
pub fn provider_from_url(url: &str) -> String {
    let host = match extract_host(url) {
        Some(h) => h,
        None => return "url".to_string(),
    };

    if host == "nexusmods.com" || host.ends_with(".nexusmods.com") {
        "nexus".to_string()
    } else if host == "modworkshop.net" || host.ends_with(".modworkshop.net") {
        "modworkshop".to_string()
    } else if host == "github.com"
        || host.ends_with(".github.com")
        || host == "objects.githubusercontent.com"
        || host.ends_with(".objects.githubusercontent.com")
    {
        "github".to_string()
    } else if host == "gamebanana.com" || host.ends_with(".gamebanana.com") {
        "gamebanana".to_string()
    } else if host == "ayakamods.com" || host.ends_with(".ayakamods.com") {
        "ayakamods".to_string()
    } else {
        "url".to_string()
    }
}

/// Extract a structured [`Source`] from a mod *page* URL (as opposed to a
/// direct download link) for the sites this manager knows the page-URL
/// shape of. Returns `None` for anything else -- callers fall back to a
/// bare `url` source in that case.
///
/// Query strings and fragments are ignored, trailing slashes don't matter,
/// and non-numeric/garbage ids are rejected rather than guessed at.
pub fn source_from_page_url(url: &str) -> Option<Source> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }

    let host = parsed.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let segments: Vec<&str> = parsed
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();

    let is_numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());

    match host {
        "ayakamods.com" => {
            // /mods/<slug>.<id>/ or /mods/<id>/
            let last = segments.get(1).filter(|_| segments.first() == Some(&"mods"))?;
            let id = last.rsplit('.').next()?;
            is_numeric(id).then(|| Source {
                provider: "ayakamods".to_string(),
                id: Some(id.to_string()),
                url: None,
                version: None,
            })
        }
        "nexusmods.com" => {
            // /helldivers2/mods/<id>
            if segments.first() != Some(&"helldivers2") || segments.get(1) != Some(&"mods") {
                return None;
            }
            let id = segments.get(2)?;
            is_numeric(id).then(|| Source {
                provider: "nexus".to_string(),
                id: Some(id.to_string()),
                url: None,
                version: None,
            })
        }
        "modworkshop.net" => {
            let id = segments.get(1).filter(|_| segments.first() == Some(&"mod"))?;
            (!id.is_empty()).then(|| Source {
                provider: "modworkshop".to_string(),
                id: Some(id.to_string()),
                url: None,
                version: None,
            })
        }
        "gamebanana.com" => {
            let id = segments.get(1).filter(|_| segments.first() == Some(&"mods"))?;
            is_numeric(id).then(|| Source {
                provider: "gamebanana".to_string(),
                id: Some(id.to_string()),
                url: None,
                version: None,
            })
        }
        "github.com" => {
            let owner = segments.first()?;
            let repo = segments.get(1)?;
            (!owner.is_empty() && !repo.is_empty()).then(|| Source {
                provider: "github".to_string(),
                id: Some(format!("{owner}/{repo}")),
                url: None,
                version: None,
            })
        }
        _ => None,
    }
}

/// Merge a mod's declared sources, its legacy `NexusData` (already converted
/// to a [`Source`] by the caller, if present), and whatever install-origin
/// sources the manager itself recorded, into the final list of resolved
/// sources shown to the user.
///
/// The legacy-NexusData source is dropped if an equivalent nexus source (same
/// id) was already declared, so old and new-style declarations never produce
/// a duplicate menu entry.
pub fn merge_sources(
    declared: Option<&[Source]>,
    legacy_nexus: Option<Source>,
    install_sources: &[Source],
) -> Vec<ResolvedSource> {
    let mut result = Vec::new();

    let declared = declared.unwrap_or(&[]);
    for source in declared {
        result.push(resolve(source));
    }

    if let Some(legacy) = legacy_nexus {
        let already_declared = declared.iter().any(|s| {
            s.provider.eq_ignore_ascii_case("nexus") && s.id == legacy.id
        });
        if !already_declared {
            result.push(resolve(&legacy));
        }
    }

    for source in install_sources {
        let mut resolved = resolve(source);
        resolved.origin = SourceOrigin::Install;
        result.push(resolved);
    }

    result
}

/// Load the `.hd2mm-origin.json` sidecar for a mod directory, if present.
///
/// A missing file is `None`; a malformed file is logged and treated as
/// `None` too -- an install-origin sidecar must never fail loading a mod.
pub async fn load_origin_sidecar(mod_dir: &Path) -> Option<OriginSidecar> {
    let path = mod_dir.join(ORIGIN_SIDECAR_FILE);
    if !path.is_file() {
        return None;
    }

    let data = match tokio::fs::read(&path).await {
        Ok(data) => data,
        Err(e) => {
            log::warn!("failed to read origin sidecar at {:?}: {}", path, e);
            return None;
        }
    };

    match serde_json::from_slice::<OriginSidecar>(&data) {
        Ok(sidecar) => Some(sidecar),
        Err(e) => {
            log::warn!("failed to parse origin sidecar at {:?}: {}", path, e);
            None
        }
    }
}

/// Write the `.hd2mm-origin.json` sidecar for a mod directory. Never touches
/// `manifest.json`.
pub async fn write_origin_sidecar(mod_dir: &Path, sources: Vec<Source>) -> anyhow::Result<()> {
    let installed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let sidecar = OriginSidecar {
        sources,
        installed_at,
    };

    let path = mod_dir.join(ORIGIN_SIDECAR_FILE);
    let data = serde_json::to_vec_pretty(&sidecar)?;
    tokio::fs::write(path, data).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(provider: &str, id: Option<&str>, url: Option<&str>) -> Source {
        Source {
            provider: provider.to_string(),
            id: id.map(str::to_string),
            url: url.map(str::to_string),
            version: None,
        }
    }

    #[test]
    fn resolve_nexus_builds_template_url() {
        let s = source("nexus", Some("123"), None);
        let r = resolve(&s);
        assert_eq!(r.display_name, "Nexus Mods");
        assert_eq!(
            r.page_url.as_deref(),
            Some("https://www.nexusmods.com/helldivers2/mods/123")
        );
        assert_eq!(r.origin, SourceOrigin::Manifest);
    }

    #[test]
    fn resolve_modworkshop_builds_template_url() {
        let s = source("modworkshop", Some("42"), None);
        let r = resolve(&s);
        assert_eq!(r.display_name, "ModWorkshop");
        assert_eq!(
            r.page_url.as_deref(),
            Some("https://modworkshop.net/mod/42")
        );
    }

    #[test]
    fn resolve_github_builds_template_url() {
        let s = source("github", Some("owner/repo"), None);
        let r = resolve(&s);
        assert_eq!(r.display_name, "GitHub");
        assert_eq!(
            r.page_url.as_deref(),
            Some("https://github.com/owner/repo")
        );
    }

    #[test]
    fn resolve_gamebanana_builds_template_url() {
        let s = source("gamebanana", Some("999"), None);
        let r = resolve(&s);
        assert_eq!(r.display_name, "GameBanana");
        assert_eq!(
            r.page_url.as_deref(),
            Some("https://gamebanana.com/mods/999")
        );
    }

    #[test]
    fn resolve_url_provider_uses_link_label() {
        let s = source("url", None, Some("https://example.com/mod"));
        let r = resolve(&s);
        assert_eq!(r.display_name, "Link");
        assert_eq!(r.page_url.as_deref(), Some("https://example.com/mod"));
    }

    #[test]
    fn resolve_unknown_provider_with_url_is_used() {
        let s = source("CoolModSite", None, Some("https://coolmodsite.example/mods/7"));
        let r = resolve(&s);
        assert_eq!(r.display_name, "CoolModSite");
        assert_eq!(
            r.page_url.as_deref(),
            Some("https://coolmodsite.example/mods/7")
        );
    }

    #[test]
    fn resolve_unknown_provider_without_url_has_no_page() {
        let s = source("CoolModSite", Some("7"), None);
        let r = resolve(&s);
        assert_eq!(r.page_url, None);
    }

    #[test]
    fn resolve_explicit_url_wins_over_template() {
        let s = source("nexus", Some("123"), Some("https://example.com/custom"));
        let r = resolve(&s);
        assert_eq!(r.page_url.as_deref(), Some("https://example.com/custom"));
    }

    #[test]
    fn resolve_rejects_javascript_scheme() {
        let s = source("url", None, Some("javascript:alert(1)"));
        let r = resolve(&s);
        assert_eq!(r.page_url, None);
    }

    #[test]
    fn resolve_rejects_file_scheme() {
        let s = source("url", None, Some("file:///etc/passwd"));
        let r = resolve(&s);
        assert_eq!(r.page_url, None);
    }

    #[test]
    fn provider_from_url_cases() {
        assert_eq!(
            provider_from_url("https://www.nexusmods.com/helldivers2/mods/1"),
            "nexus"
        );
        assert_eq!(
            provider_from_url("https://modworkshop.net/mod/1"),
            "modworkshop"
        );
        assert_eq!(provider_from_url("https://github.com/owner/repo"), "github");
        assert_eq!(
            provider_from_url("https://objects.githubusercontent.com/foo"),
            "github"
        );
        assert_eq!(
            provider_from_url("https://gamebanana.com/mods/1"),
            "gamebanana"
        );
        assert_eq!(
            provider_from_url("https://cdn.example.com/download.zip"),
            "url"
        );
        assert_eq!(provider_from_url("not a url"), "url");
    }

    #[test]
    fn merge_sources_dedupes_legacy_nexus_against_declared() {
        let declared = vec![source("nexus", Some("123"), None)];
        let legacy = Some(source("nexus", Some("123"), None));
        let result = merge_sources(Some(&declared), legacy, &[]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn merge_sources_keeps_legacy_nexus_when_not_declared() {
        let declared = vec![source("github", Some("owner/repo"), None)];
        let legacy = Some(source("nexus", Some("123"), None));
        let result = merge_sources(Some(&declared), legacy, &[]);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|r| r.display_name == "Nexus Mods"));
    }

    #[test]
    fn merge_sources_marks_install_origin() {
        let install = vec![source("url", None, Some("https://example.com/x.zip"))];
        let result = merge_sources(None, None, &install);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].origin, SourceOrigin::Install);
    }

    #[tokio::test]
    async fn origin_sidecar_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let sources = vec![source("url", None, Some("https://example.com/x.zip"))];
        write_origin_sidecar(dir.path(), sources).await.unwrap();

        let loaded = load_origin_sidecar(dir.path()).await;
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert!(loaded.installed_at > 0);
    }

    #[tokio::test]
    async fn origin_sidecar_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_origin_sidecar(dir.path()).await;
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn origin_sidecar_malformed_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join(ORIGIN_SIDECAR_FILE), b"not json")
            .await
            .unwrap();
        let loaded = load_origin_sidecar(dir.path()).await;
        assert!(loaded.is_none());
    }

    #[test]
    fn resolve_ayakamods_builds_template_url() {
        let s = source("ayakamods", Some("4084"), None);
        let r = resolve(&s);
        assert_eq!(r.display_name, "AyakaMods");
        assert_eq!(
            r.page_url.as_deref(),
            Some("https://ayakamods.com/mods/4084/")
        );
    }

    #[test]
    fn provider_from_url_detects_ayakamods() {
        assert_eq!(
            provider_from_url("https://ayakamods.com/mods/hd2-auto-reload.4084/"),
            "ayakamods"
        );
        assert_eq!(
            provider_from_url("https://www.ayakamods.com/mods/hd2-auto-reload.4084/"),
            "ayakamods"
        );
    }

    #[test]
    fn source_from_page_url_ayakamods_slug_and_id() {
        let s = source_from_page_url("https://ayakamods.com/mods/hd2-auto-reload.4084/").unwrap();
        assert_eq!(s.provider, "ayakamods");
        assert_eq!(s.id.as_deref(), Some("4084"));
    }

    #[test]
    fn source_from_page_url_ayakamods_bare_id_no_trailing_slash() {
        let s = source_from_page_url("https://ayakamods.com/mods/4084").unwrap();
        assert_eq!(s.provider, "ayakamods");
        assert_eq!(s.id.as_deref(), Some("4084"));
    }

    #[test]
    fn source_from_page_url_ayakamods_with_query_and_fragment() {
        let s = source_from_page_url(
            "https://www.ayakamods.com/mods/hd2-auto-reload.4084/?utm_source=x#reviews",
        )
        .unwrap();
        assert_eq!(s.provider, "ayakamods");
        assert_eq!(s.id.as_deref(), Some("4084"));
    }

    #[test]
    fn source_from_page_url_ayakamods_rejects_non_numeric_id() {
        assert!(source_from_page_url("https://ayakamods.com/mods/hd2-auto-reload.abc/").is_none());
        assert!(source_from_page_url("https://ayakamods.com/mods/not-a-number/").is_none());
    }

    #[test]
    fn source_from_page_url_ayakamods_download_subpath_still_resolves() {
        let s =
            source_from_page_url("https://ayakamods.com/mods/hd2-auto-reload.4084/download")
                .unwrap();
        assert_eq!(s.id.as_deref(), Some("4084"));
    }

    #[test]
    fn source_from_page_url_nexus() {
        let s =
            source_from_page_url("https://www.nexusmods.com/helldivers2/mods/123?tab=files")
                .unwrap();
        assert_eq!(s.provider, "nexus");
        assert_eq!(s.id.as_deref(), Some("123"));
    }

    #[test]
    fn source_from_page_url_nexus_rejects_non_numeric() {
        assert!(source_from_page_url("https://www.nexusmods.com/helldivers2/mods/abc").is_none());
    }

    #[test]
    fn source_from_page_url_modworkshop() {
        let s = source_from_page_url("https://modworkshop.net/mod/12345/").unwrap();
        assert_eq!(s.provider, "modworkshop");
        assert_eq!(s.id.as_deref(), Some("12345"));
    }

    #[test]
    fn source_from_page_url_gamebanana() {
        let s = source_from_page_url("https://gamebanana.com/mods/999999").unwrap();
        assert_eq!(s.provider, "gamebanana");
        assert_eq!(s.id.as_deref(), Some("999999"));
    }

    #[test]
    fn source_from_page_url_gamebanana_rejects_non_numeric() {
        assert!(source_from_page_url("https://gamebanana.com/mods/abc").is_none());
    }

    #[test]
    fn source_from_page_url_github() {
        let s = source_from_page_url("https://github.com/someone/example-mod").unwrap();
        assert_eq!(s.provider, "github");
        assert_eq!(s.id.as_deref(), Some("someone/example-mod"));
    }

    #[test]
    fn source_from_page_url_github_with_extra_path_segments() {
        let s = source_from_page_url("https://github.com/someone/example-mod/releases/latest")
            .unwrap();
        assert_eq!(s.id.as_deref(), Some("someone/example-mod"));
    }

    #[test]
    fn source_from_page_url_unknown_host_is_none() {
        assert!(source_from_page_url("https://example.com/downloads/mod.zip").is_none());
    }

    #[test]
    fn source_from_page_url_rejects_non_http_scheme() {
        assert!(source_from_page_url("ftp://ayakamods.com/mods/4084/").is_none());
    }

    #[test]
    fn source_from_page_url_rejects_garbage() {
        assert!(source_from_page_url("not a url at all").is_none());
    }
}
