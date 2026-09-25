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
    /// Which exact file (of possibly several on the mod page) was installed,
    /// per provider, when DDMM knows -- lets update checks follow that file
    /// rather than guessing, and lets a one-click update pick the matching
    /// new file. Optional; older sidecars simply don't have it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed_files: Vec<InstalledFile>,
    /// "Skip this version": a provider version the user chose not to be
    /// told about again. Cleared whenever the mod is (re)installed or
    /// updated, since the sidecar is rewritten then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_versions: Vec<SkippedVersion>,
}

/// The specific file of a mod page that was installed (see
/// [`OriginSidecar::installed_files`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InstalledFile {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// The file's title/label on the site (e.g. a Nexus file's name or a
    /// GameBanana file description) -- more stable across versions than the
    /// archive name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Upload time (Unix seconds) as reported by the site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SkippedVersion {
    pub provider: String,
    pub version: String,
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
    write_origin_sidecar_with_files(mod_dir, sources, Vec::new()).await
}

/// [`write_origin_sidecar`], also recording which file(s) were installed.
pub async fn write_origin_sidecar_with_files(
    mod_dir: &Path,
    sources: Vec<Source>,
    installed_files: Vec<InstalledFile>,
) -> anyhow::Result<()> {
    let installed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let sidecar = OriginSidecar {
        sources,
        installed_at,
        installed_files,
        skipped_versions: Vec::new(),
    };
    save_origin_sidecar(mod_dir, &sidecar).await
}

/// Write `sidecar` as-is (used to update skip lists without touching the
/// recorded sources).
pub async fn save_origin_sidecar(mod_dir: &Path, sidecar: &OriginSidecar) -> anyhow::Result<()> {
    let path = mod_dir.join(ORIGIN_SIDECAR_FILE);
    let data = serde_json::to_vec_pretty(sidecar)?;
    tokio::fs::write(path, data).await?;
    Ok(())
}

/// Set (`Some`) or clear (`None`) the skipped version for `provider` in a
/// mod's sidecar, creating an otherwise-empty sidecar if the mod has none
/// (a mod whose sources all come from its own manifest).
pub async fn set_skipped_version(mod_dir: &Path, provider: &str, version: Option<&str>) -> anyhow::Result<()> {
    let mut sidecar = match load_origin_sidecar(mod_dir).await {
        Some(s) => s,
        None => OriginSidecar {
            sources: Vec::new(),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            installed_files: Vec::new(),
            skipped_versions: Vec::new(),
        },
    };
    sidecar
        .skipped_versions
        .retain(|s| !s.provider.eq_ignore_ascii_case(provider));
    if let Some(version) = version {
        sidecar.skipped_versions.push(SkippedVersion {
            provider: provider.to_string(),
            version: version.to_string(),
        });
    }
    save_origin_sidecar(mod_dir, &sidecar).await
}

/// The raw provider id (AyakaMods/Nexus/GameBanana numeric id, ModWorkshop
/// id, or `owner/repo` for GitHub) behind a resolved source, recovered from
/// its page URL the same way a pasted page URL is parsed. `ResolvedSource`
/// doesn't carry the id itself (it's a frontend-facing type).
pub fn resolved_source_id(source: &ResolvedSource) -> Option<String> {
    let page_url = source.page_url.as_ref()?;
    let parsed = source_from_page_url(page_url)?;
    if !parsed.provider.eq_ignore_ascii_case(&source.provider) {
        return None;
    }
    parsed.id
}

/// What a Nexus Mods download's file name says about it. Nexus names every
/// download `<name>-<modId>-<version with dashes>-<uploadUnixTime>.<ext>`
/// (browsers may append ` (1)` for a duplicate), which is enough to record
/// the installed version and match the exact file later -- no API call,
/// no key needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NexusArchiveName {
    pub mod_id: String,
    /// Best-effort: dashes turned back into dots (Nexus replaces both with
    /// dashes, so "1.0-beta" comes back as "1.0.beta"; version comparison
    /// treats `-` and `.` alike for this reason).
    pub version: String,
    pub uploaded_at: i64,
}

pub fn parse_nexus_archive_name(file_name: &str) -> Option<NexusArchiveName> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)^.+?-(\d+)-([0-9a-z][0-9a-z-]*?)-(\d{9,11})(?: \(\d+\))?\.(?:zip|7z|rar)$",
        )
        .unwrap()
    });
    let caps = re.captures(file_name)?;
    Some(NexusArchiveName {
        mod_id: caps.get(1)?.as_str().to_string(),
        version: caps.get(2)?.as_str().replace('-', "."),
        uploaded_at: caps.get(3)?.as_str().parse().ok()?,
    })
}

/// The release tag in a GitHub release-asset download URL
/// (`https://github.com/<owner>/<repo>/releases/download/<tag>/<file>`).
pub fn github_tag_from_download_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let segments: Vec<&str> = parsed.path_segments()?.collect();
    match segments.as_slice() {
        [_, _, "releases", "download", tag, file] if !tag.is_empty() && !file.is_empty() => {
            Some(percent_encoding::percent_decode_str(tag).decode_utf8_lossy().into_owned())
        }
        _ => None,
    }
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
    fn parses_nexus_archive_names() {
        let n = parse_nexus_archive_name("Better Stims-1234-1-2-0-1718000000.zip").unwrap();
        assert_eq!(n.mod_id, "1234");
        assert_eq!(n.version, "1.2.0");
        assert_eq!(n.uploaded_at, 1718000000);

        let dup = parse_nexus_archive_name("Better Stims-1234-2-0-1718000000 (1).7z").unwrap();
        assert_eq!(dup.version, "2.0");

        let beta = parse_nexus_archive_name("Mod-with-dashes-77-1-0-beta-1718000000.rar").unwrap();
        assert_eq!(beta.mod_id, "77");
        assert_eq!(beta.version, "1.0.beta");

        assert!(parse_nexus_archive_name("cool-mod.zip").is_none());
        assert!(parse_nexus_archive_name("Test Mod-4084-1-0.zip").is_none());
        assert!(parse_nexus_archive_name("x-1-1-1718000000.exe").is_none());
    }

    #[test]
    fn github_tag_from_asset_urls() {
        assert_eq!(
            github_tag_from_download_url("https://github.com/o/r/releases/download/v1.2.3/mod.zip").as_deref(),
            Some("v1.2.3")
        );
        assert_eq!(
            github_tag_from_download_url("https://github.com/o/r/releases/download/release%2F2/mod.zip").as_deref(),
            Some("release/2")
        );
        assert!(github_tag_from_download_url("https://github.com/o/r/archive/refs/heads/main.zip").is_none());
        assert!(github_tag_from_download_url("https://example.com/o/r/releases/download/v1/x.zip").is_none());
    }

    #[test]
    fn resolved_source_id_requires_matching_provider() {
        let r = resolve(&source("gamebanana", Some("42"), None));
        assert_eq!(resolved_source_id(&r).as_deref(), Some("42"));
        // A "nexus" source whose explicit Url points somewhere else entirely.
        let r = resolve(&source("nexus", Some("1"), Some("https://gamebanana.com/mods/9")));
        assert_eq!(resolved_source_id(&r), None);
    }

    #[tokio::test]
    async fn old_sidecars_without_new_fields_still_load() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join(ORIGIN_SIDECAR_FILE),
            br#"{"Sources":[{"Provider":"nexus","Id":"5","Version":"1.0"}],"InstalledAt":1}"#,
        )
        .await
        .unwrap();
        let loaded = load_origin_sidecar(dir.path()).await.unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert!(loaded.installed_files.is_empty());
        assert!(loaded.skipped_versions.is_empty());
    }

    #[tokio::test]
    async fn skipped_version_is_set_replaced_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        write_origin_sidecar(dir.path(), vec![source("github", Some("o/r"), None)]).await.unwrap();

        set_skipped_version(dir.path(), "github", Some("v2")).await.unwrap();
        set_skipped_version(dir.path(), "github", Some("v3")).await.unwrap();
        let s = load_origin_sidecar(dir.path()).await.unwrap();
        assert_eq!(s.skipped_versions, vec![SkippedVersion { provider: "github".into(), version: "v3".into() }]);
        assert_eq!(s.sources.len(), 1, "skipping must not touch the recorded sources");

        set_skipped_version(dir.path(), "github", None).await.unwrap();
        assert!(load_origin_sidecar(dir.path()).await.unwrap().skipped_versions.is_empty());
    }

    #[tokio::test]
    async fn skipping_creates_a_sidecar_when_none_exists() {
        let dir = tempfile::tempdir().unwrap();
        set_skipped_version(dir.path(), "nexus", Some("2.0")).await.unwrap();
        let s = load_origin_sidecar(dir.path()).await.unwrap();
        assert!(s.sources.is_empty());
        assert_eq!(s.skipped_versions.len(), 1);
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
