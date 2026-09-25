//! Per-site update checking: each submodule knows how to ask one mod site
//! "what's the latest version of mod X, and (if the site allows it) where
//! can the new file be downloaded directly?".
//!
//! Only public, documented endpoints are used, with a clear User-Agent,
//! timeouts, per-host pacing and size caps. Keyless for every site except
//! Nexus Mods, which needs the user's own optional API key (see
//! [`nexus`] and `crate::secrets`) and is never used to download anything.

pub mod ayakamods;
pub mod gamebanana;
pub mod github;
pub mod modworkshop;
pub mod nexus;

use std::{collections::HashMap, time::Duration};

use futures::StreamExt;
use serde::{Deserialize, Serialize};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Cap on any single API response / page we read (not on mod downloads,
/// which have their own limit in `crate::download`).
pub const MAX_RESPONSE_SIZE: u64 = 5 * 1024 * 1024;

pub const APP_NAME: &str = "Democracy Defender Mod Manager";

pub fn user_agent() -> String {
    format!(
        "DemocracyDefenderModManager/{} (+https://github.com/katsyk/DemocracyDefenderModManager)",
        env!("CARGO_PKG_VERSION")
    )
}

/// The shared client for keyless update checks.
pub fn build_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .https_only(true)
        .build()?)
}

/// A file a one-click update could download directly -- only ever offered
/// for sites that serve the new version publicly, without a login or key
/// (GitHub release assets, GameBanana and ModWorkshop files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateFile {
    /// Site-specific file id (asset id, file id), as a string.
    pub id: String,
    /// The archive's file name.
    pub name: String,
    /// A human label for the file when the site has one (GameBanana file
    /// description, ModWorkshop file label/name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<i64>,
    /// Direct https URL. Re-validated against the provider's host
    /// allowlist ([`is_allowed_download_host`]) before anything downloads it.
    pub url: String,
}

/// Hosts a direct (one-click) update may *start* downloading from, per
/// provider. Redirects afterwards (to a CDN) are fine -- the point is that
/// a URL handed back from the frontend can't be pointed at an arbitrary
/// site.
pub fn is_allowed_download_host(provider: &str, url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str().map(|h| h.to_ascii_lowercase()) else { return false };
    match provider {
        "github" => host == "github.com" || host == "objects.githubusercontent.com",
        "gamebanana" => host == "gamebanana.com" || host == "files.gamebanana.com",
        "modworkshop" => host == "api.modworkshop.net" || host == "storage.modworkshop.net",
        _ => false,
    }
}

pub fn has_archive_extension(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zip") || lower.ends_with(".7z") || lower.ends_with(".rar")
}

/// GET `url` (https only) and read at most [`MAX_RESPONSE_SIZE`] bytes of
/// it (checked against `Content-Length` and the streamed size).
pub async fn fetch_capped(client: &reqwest::Client, url: &str) -> anyhow::Result<(String, reqwest::header::HeaderMap)> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "https" {
        anyhow::bail!("only https:// URLs are supported");
    }
    let host = parsed.host_str().unwrap_or("the server").to_string();

    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("couldn't reach {host}: {}", e.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{host} answered {status}");
    }
    let headers = response.headers().clone();
    let body = read_capped(response).await?;
    Ok((body, headers))
}

pub async fn read_capped(response: reqwest::Response) -> anyhow::Result<String> {
    if let Some(len) = response.content_length() {
        if len > MAX_RESPONSE_SIZE {
            anyhow::bail!("response is {len} bytes, which exceeds the {MAX_RESPONSE_SIZE} byte limit");
        }
    }
    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("reading the response failed: {}", e.without_url()))?;
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > MAX_RESPONSE_SIZE {
            anyhow::bail!("response exceeded the {MAX_RESPONSE_SIZE} byte limit while streaming");
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Keeps at least a minimum gap between requests to the same host, so a
/// check across many mods never bursts at any one site.
#[derive(Default)]
pub struct Pacer {
    last: HashMap<&'static str, tokio::time::Instant>,
}

impl Pacer {
    pub async fn wait(&mut self, host: &'static str, gap: Duration) {
        if let Some(last) = self.last.get(host) {
            let elapsed = last.elapsed();
            if elapsed < gap {
                tokio::time::sleep(gap - elapsed).await;
            }
        }
        self.last.insert(host, tokio::time::Instant::now());
    }
}

/// How an installed version relates to the latest one a site reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRelation {
    Same,
    /// The site has something different -- newer, or at least not what's
    /// installed. (Most mod versions aren't comparable numerically, so
    /// "different" is treated as "update available", except below.)
    Update,
    /// Both are plain dotted numbers and the installed one is *higher*
    /// (e.g. a pre-release installed by hand) -- not an update.
    InstalledNewer,
}

/// Normalize for comparison: trim, drop a leading `v`, lowercase, and
/// treat `-`/`_` like `.` (Nexus file names turn dots into dashes).
fn normalize_version(v: &str) -> String {
    let v = v.trim();
    let v = v.strip_prefix(['v', 'V']).filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit())).unwrap_or(v);
    v.to_ascii_lowercase().replace(['-', '_'], ".")
}

fn numeric_parts(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

pub fn compare_versions(installed: &str, latest: &str) -> VersionRelation {
    let a = normalize_version(installed);
    let b = normalize_version(latest);
    if a == b {
        return VersionRelation::Same;
    }
    if let (Some(mut x), Some(mut y)) = (numeric_parts(&a), numeric_parts(&b)) {
        let n = x.len().max(y.len());
        x.resize(n, 0);
        y.resize(n, 0);
        return match x.cmp(&y) {
            std::cmp::Ordering::Equal => VersionRelation::Same,
            std::cmp::Ordering::Less => VersionRelation::Update,
            std::cmp::Ordering::Greater => VersionRelation::InstalledNewer,
        };
    }
    VersionRelation::Update
}

/// Format Unix seconds as `YYYY-MM-DD HH:MM UTC` -- used as the "version"
/// of files/mods on sites whose authors didn't set one, so there's still
/// something honest to compare and show.
pub fn format_timestamp(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, (secs % 3600) / 60)
}

/// Parse an RFC 3339 / ISO-8601 UTC timestamp like
/// `2026-04-30T04:30:31.000000Z` into Unix seconds (just enough for the
/// sites here; no timezone offsets other than `Z`).
pub fn parse_iso_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once('T')?;
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    let time = time.trim_end_matches('Z');
    let time = time.split('.').next()?;
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next()?.parse().ok()?;
    let ss: i64 = tp.next().unwrap_or("0").parse().ok()?;
    // Days from civil (inverse of the above).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// A comparable "shape" of a file name or label: lowercase, extension and
/// digits/separators stripped, and GameBanana's random 5-hex upload suffix
/// removed -- so `cool_mod_v1_ab12c.zip` and `cool_mod_v2_ff3bb.zip` match.
pub fn file_shape(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stem = ["zip", "7z", "rar"]
        .iter()
        .find_map(|ext| lower.strip_suffix(&format!(".{ext}")))
        .unwrap_or(&lower)
        .to_string();
    static GB_SUFFIX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = GB_SUFFIX.get_or_init(|| regex::Regex::new(r"_[0-9a-f]{5}$").unwrap());
    let stem = re.replace(&stem, "");
    stem.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .trim_start_matches('v')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert_eq!(compare_versions("1.0", "1.0"), VersionRelation::Same);
        assert_eq!(compare_versions("v1.2.0", "1.2"), VersionRelation::Same);
        assert_eq!(compare_versions("1-2-0", "1.2.0"), VersionRelation::Same);
        assert_eq!(compare_versions("1.0", "1.1"), VersionRelation::Update);
        assert_eq!(compare_versions("1.10", "1.9"), VersionRelation::InstalledNewer);
        assert_eq!(compare_versions("2026-09-24", "2026-09-25"), VersionRelation::Update);
        assert_eq!(compare_versions("beta", "gamma"), VersionRelation::Update);
        assert_eq!(compare_versions(" V3 ", "v3"), VersionRelation::Same);
    }

    #[test]
    fn timestamps_round_trip() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_timestamp(1777441761), "2026-04-29 05:49 UTC");
        assert_eq!(parse_iso_utc("2026-04-30T04:30:31.000000Z"), Some(1777523431));
        assert_eq!(parse_iso_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_utc("garbage"), None);
    }

    #[test]
    fn download_host_allowlist() {
        assert!(is_allowed_download_host("github", "https://github.com/o/r/releases/download/v1/a.zip"));
        assert!(is_allowed_download_host("gamebanana", "https://gamebanana.com/dl/1"));
        assert!(is_allowed_download_host("modworkshop", "https://api.modworkshop.net/files/1/download"));
        assert!(!is_allowed_download_host("github", "http://github.com/o/r/releases/download/v1/a.zip"));
        assert!(!is_allowed_download_host("github", "https://evil.example/a.zip"));
        assert!(!is_allowed_download_host("gamebanana", "https://github.com/x.zip"));
        assert!(!is_allowed_download_host("nexus", "https://www.nexusmods.com/x"));
        assert!(!is_allowed_download_host("ayakamods", "https://ayakamods.com/x"));
    }

    #[test]
    fn file_shapes_ignore_versions_and_upload_suffixes() {
        assert_eq!(file_shape("rabu_ss_sa-8_no_helm_8430d.zip"), file_shape("rabu_ss_sa-9_no_helm_ff3bb.zip"));
        assert_ne!(file_shape("rabu_ss_sa-8_no_helm_8430d.zip"), file_shape("rabu_ss_dp-00_1acbd.zip"));
        assert_eq!(file_shape("CoolMod-v1.2.zip"), file_shape("CoolMod-v1.3.zip"));
    }
}
