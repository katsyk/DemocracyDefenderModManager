//! Nexus Mods update checks through the public v1 API, using the user's
//! *optional* Nexus Mods sign-in (OAuth, see `crate::nexus_oauth`) or
//! personal API key (see `crate::secrets`). Which one is used is decided in
//! one place, [`crate::nexus_oauth::resolve_auth`]. Without either, Nexus
//! mods are reported as "optional: sign in or add a key to check" -- never
//! guessed.
//!
//! What this module will and won't do:
//! - It only ever talks to `https://api.nexusmods.com` (fixed base URL,
//!   redirects disabled so the key header can't follow one anywhere else).
//! - It sends the headers Nexus's API Acceptable Use Policy asks for
//!   (`Authorization: Bearer <token>` when signed in, else `apikey`; plus
//!   `Application-Name` and `Application-Version`).
//! - It reads the `x-rl-*` rate-limit headers on every response and stops
//!   early rather than run a user's quota down.
//! - It never downloads anything. Nexus's download-link endpoint is
//!   premium-only and deliberately unused; updating a Nexus mod always goes
//!   through the user's browser, where they click Nexus's own download
//!   button themselves (DDMM never automates "Slow download").
//!
//! Request budget per check follows Mod Organizer 2's `checkAllForUpdate`
//! (see [`plan`]): mods checked in the last 5 minutes are skipped; mods
//! never checked (or not within a month) get one `files.json` request
//! each; the rest share one `updated.json?period=1d|1w|1m` request (the
//! smallest window covering the oldest check), and only the mods it lists
//! as changed get a `files.json` request. Last-check times and results
//! live in `update-cache.json` in the data folder.
//!
//! Only user-initiated checks call the API at all (Nexus's API Acceptable
//! Use Policy); the opt-in automatic checks skip Nexus mods.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::secrets::{redact_all, NexusApiKey, Secret};

use super::{compare_versions, file_shape, read_capped, VersionRelation, APP_NAME};

pub const API_BASE: &str = "https://api.nexusmods.com/v1/";
pub const API_HOST: &str = "api.nexusmods.com";
/// Helldivers 2's Nexus game domain (www.nexusmods.com/helldivers2).
pub const GAME_DOMAIN: &str = "helldivers2";
/// Gap between Nexus requests within one check. The API allows far more;
/// this just keeps a check over many mods from bursting.
pub const REQUEST_GAP: Duration = Duration::from_millis(300);
/// Stop making requests once either quota drops below this.
pub const QUOTA_FLOOR: u32 = 10;

const CACHE_FILE: &str = "update-cache.json";

/// Nexus file categories (`category_id`).
const CATEGORY_MAIN: u32 = 1;
const CATEGORY_UPDATE: u32 = 2;
const CATEGORY_OPTIONAL: u32 = 3;
const CATEGORY_OLD_VERSION: u32 = 4;
const CATEGORY_MISC: u32 = 5;

/// How DDMM authenticates to the Nexus Mods API. Built only by
/// `crate::nexus_oauth::resolve_auth` (and tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NexusAuth {
    /// "Sign in to Nexus Mods": an OAuth access token, sent as
    /// `Authorization: Bearer <token>`.
    OAuth(Secret),
    /// A personal API key entered manually, sent as `apikey`.
    ApiKey(NexusApiKey),
}

impl NexusAuth {
    fn secret(&self) -> &str {
        match self {
            NexusAuth::OAuth(token) => token.expose(),
            NexusAuth::ApiKey(key) => key.expose(),
        }
    }

    pub fn is_oauth(&self) -> bool {
        matches!(self, NexusAuth::OAuth(_))
    }
}

impl From<NexusApiKey> for NexusAuth {
    fn from(key: NexusApiKey) -> Self {
        NexusAuth::ApiKey(key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NexusError {
    /// 401/403: the key or sign-in is wrong, revoked, or expired.
    InvalidKey,
    /// 429, or our own floor on the remaining quota was reached.
    RateLimited,
    NotFound,
    Other(String),
}

impl std::fmt::Display for NexusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NexusError::InvalidKey => f.write_str("Nexus Mods didn't accept the API key (it may have been revoked)"),
            NexusError::RateLimited => f.write_str("the Nexus Mods API request limit for your key is nearly used up; try again later"),
            NexusError::NotFound => f.write_str("Nexus Mods doesn't have this mod (it may have been removed or hidden)"),
            NexusError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for NexusError {}

/// Remaining request quota, from the last response's `x-rl-*` headers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RateLimit {
    pub hourly_remaining: Option<u32>,
    pub daily_remaining: Option<u32>,
}

impl RateLimit {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
        RateLimit {
            hourly_remaining: get("x-rl-hourly-remaining"),
            daily_remaining: get("x-rl-daily-remaining"),
        }
    }

    /// Nexus grants a daily allowance and, once that's spent, an hourly
    /// one; we stop when *both* are nearly gone -- or either, to be
    /// conservative about quota we don't know the accounting of.
    pub fn is_low(&self) -> bool {
        self.hourly_remaining.is_some_and(|h| h < QUOTA_FLOOR) || self.daily_remaining.is_some_and(|d| d < QUOTA_FLOOR)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidatedUser {
    pub name: String,
    #[serde(default)]
    pub is_premium: bool,
    // The real response also echoes the key and the account email; they
    // are deliberately not deserialized, so they can't be stored or logged.
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UpdatedEntry {
    pub mod_id: u64,
    #[serde(default)]
    pub latest_file_update: i64,
    #[serde(default)]
    pub latest_mod_activity: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModFiles {
    #[serde(default)]
    pub files: Vec<NexusFile>,
    #[serde(default)]
    pub file_updates: Vec<FileUpdate>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NexusFile {
    pub file_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub mod_version: Option<String>,
    #[serde(default)]
    pub category_id: Option<u32>,
    #[serde(default)]
    pub category_name: Option<String>,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub uploaded_timestamp: Option<i64>,
}

impl NexusFile {
    fn is_current(&self) -> bool {
        matches!(
            self.category_id,
            Some(CATEGORY_MAIN | CATEGORY_UPDATE | CATEGORY_OPTIONAL | CATEGORY_MISC)
        ) || (self.category_id.is_none()
            && matches!(self.category_name.as_deref(), Some("MAIN" | "UPDATE" | "OPTIONAL" | "MISCELLANEOUS")))
    }

    fn is_main(&self) -> bool {
        self.category_id == Some(CATEGORY_MAIN) || self.category_name.as_deref() == Some("MAIN")
    }

    fn is_old(&self) -> bool {
        !self.is_current() || self.category_id == Some(CATEGORY_OLD_VERSION)
    }

    fn display_version(&self) -> Option<String> {
        self.version
            .clone()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| self.mod_version.clone().filter(|v| !v.trim().is_empty()))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FileUpdate {
    pub old_file_id: u64,
    pub new_file_id: u64,
    #[serde(default)]
    pub uploaded_timestamp: Option<i64>,
}

pub struct NexusClient {
    http: reqwest::Client,
    base: reqwest::Url,
    auth: NexusAuth,
    pub rate: RateLimit,
}

impl NexusClient {
    pub fn new(auth: NexusAuth) -> anyhow::Result<Self> {
        Self::build(auth, API_BASE, true)
    }

    /// Test-only: point at a local mock (plain http allowed).
    #[cfg(test)]
    pub(crate) fn with_base(auth: NexusAuth, base: &str) -> anyhow::Result<Self> {
        Self::build(auth, base, false)
    }

    pub fn auth(&self) -> &NexusAuth {
        &self.auth
    }

    fn build(auth: NexusAuth, base: &str, https_only: bool) -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        let (name, value) = match &auth {
            NexusAuth::OAuth(token) => (reqwest::header::AUTHORIZATION.as_str(), format!("Bearer {}", token.expose())),
            NexusAuth::ApiKey(key) => ("apikey", key.expose().to_string()),
        };
        let mut key_value = HeaderValue::from_str(&value)
            .map_err(|_| anyhow::anyhow!("the key contains characters that can't be sent"))?;
        key_value.set_sensitive(true);
        headers.insert(name, key_value);
        headers.insert("Application-Name", HeaderValue::from_static(APP_NAME));
        headers.insert("Application-Version", HeaderValue::from_static(env!("CARGO_PKG_VERSION")));
        headers.insert(reqwest::header::ACCEPT, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .user_agent(super::user_agent())
            .default_headers(headers)
            // Never follow a redirect: the `apikey`/`Authorization` header
            // must only ever reach api.nexusmods.com.
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .timeout(super::REQUEST_TIMEOUT)
            .connect_timeout(super::CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base: reqwest::Url::parse(base)?,
            auth,
            rate: RateLimit::default(),
        })
    }

    async fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T, NexusError> {
        if self.rate.is_low() {
            return Err(NexusError::RateLimited);
        }
        let url = self.base.join(path).map_err(|e| NexusError::Other(e.to_string()))?;
        if url.host_str() != self.base.host_str() || url.scheme() != self.base.scheme() {
            return Err(NexusError::Other("refusing to send Nexus Mods credentials to another host".into()));
        }

        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| NexusError::Other(redact_all(&format!("couldn't reach Nexus Mods: {}", e.without_url()), &[self.auth.secret()])))?;

        self.rate = RateLimit::from_headers(response.headers());
        let status = response.status();
        match status.as_u16() {
            200..=299 => {}
            401 | 403 => return Err(NexusError::InvalidKey),
            404 => return Err(NexusError::NotFound),
            429 => return Err(NexusError::RateLimited),
            _ => return Err(NexusError::Other(format!("Nexus Mods answered {status}"))),
        }
        let body = read_capped(response)
            .await
            .map_err(|e| NexusError::Other(redact_all(&e.to_string(), &[self.auth.secret()])))?;
        serde_json::from_str(&body)
            .map_err(|e| NexusError::Other(redact_all(&format!("unexpected response from Nexus Mods: {e}"), &[self.auth.secret()])))
    }

    /// `GET /v1/users/validate.json` -- is the key valid, and whose is it?
    pub async fn validate(&mut self) -> Result<ValidatedUser, NexusError> {
        self.get("users/validate.json").await
    }

    /// `GET /v1/games/helldivers2/mods/updated.json?period=1w|1m`
    pub async fn updated(&mut self, period: Period) -> Result<Vec<UpdatedEntry>, NexusError> {
        self.get(&format!("games/{GAME_DOMAIN}/mods/updated.json?period={}", period.as_param()))
            .await
    }

    /// `GET /v1/games/helldivers2/mods/{id}/files.json` (all categories, so
    /// an installed file that's since become an "old version" can still be
    /// matched).
    pub async fn files(&mut self, mod_id: u64) -> Result<ModFiles, NexusError> {
        self.get(&format!("games/{GAME_DOMAIN}/mods/{mod_id}/files.json")).await
    }
}

/// What DDMM knows about the installed copy of a Nexus mod.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installed {
    pub version: Option<String>,
    pub file_id: Option<u64>,
    pub uploaded_at: Option<i64>,
    pub file_name: Option<String>,
}

impl Installed {
    /// A string that changes whenever what's installed changes (so a cached
    /// decision is never reused for a different installed file).
    pub fn cache_key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.version.as_deref().unwrap_or(""),
            self.file_id.map(|i| i.to_string()).unwrap_or_default(),
            self.uploaded_at.map(|i| i.to_string()).unwrap_or_default(),
            self.file_name.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionState {
    UpToDate,
    Update,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Decision {
    pub state: DecisionState,
    /// The installed file's real version, when it could be matched.
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    /// Title of the newer file (for multi-file mods).
    pub latest_file_name: Option<String>,
}

/// Decide whether an installed Nexus file has an update, from the mod's
/// file list. Pure -- see the tests.
///
/// 1. Find the installed file: by recorded file id, else by upload time
///    (from the download's own file name), else by archive name.
/// 2. If found, follow the author's "newer version of this file" links
///    (`file_updates`) to the end; a different file there is the update.
///    An installed file that's since been moved to "old versions" with no
///    link is matched to the newest current file with the same title.
/// 3. If not found but a version is recorded, compare it with the mod's
///    primary/main file.
/// 4. Otherwise: unknown (never guessed).
pub fn decide(installed: &Installed, files: &ModFiles) -> Decision {
    let by_id: HashMap<u64, &NexusFile> = files.files.iter().map(|f| (f.file_id, f)).collect();

    let resolved = installed
        .file_id
        .and_then(|id| by_id.get(&id).copied())
        .or_else(|| {
            installed
                .uploaded_at
                .and_then(|ts| files.files.iter().find(|f| f.uploaded_timestamp == Some(ts)))
        })
        .or_else(|| {
            installed.file_name.as_deref().and_then(|name| {
                files.files.iter().find(|f| !f.file_name.is_empty() && f.file_name.eq_ignore_ascii_case(name))
            })
        });

    let current_main = || {
        files
            .files
            .iter()
            .filter(|f| f.is_current())
            .max_by_key(|f| (f.is_primary, f.is_main(), f.uploaded_timestamp.unwrap_or(0)))
    };

    let update_to = |installed_version: Option<String>, newer: &NexusFile| Decision {
        state: DecisionState::Update,
        installed_version,
        latest_version: newer.display_version(),
        latest_file_name: Some(newer.name.clone()),
    };

    if let Some(file) = resolved {
        let installed_version = file.display_version().or_else(|| installed.version.clone());

        // Follow the chain of "new version of this file" links.
        let mut current = file.file_id;
        let mut seen = HashSet::from([current]);
        while let Some(next) = files
            .file_updates
            .iter()
            .filter(|u| u.old_file_id == current)
            .max_by_key(|u| u.uploaded_timestamp.unwrap_or(0))
        {
            if !seen.insert(next.new_file_id) {
                break;
            }
            current = next.new_file_id;
        }
        if current != file.file_id {
            if let Some(newer) = by_id.get(&current) {
                return update_to(installed_version, newer);
            }
        }

        if file.is_old() {
            let shape = file_shape(&file.name);
            let same_title = files
                .files
                .iter()
                .filter(|f| f.is_current() && f.file_id != file.file_id && file_shape(&f.name) == shape)
                .filter(|f| f.uploaded_timestamp.unwrap_or(0) > file.uploaded_timestamp.unwrap_or(0))
                .max_by_key(|f| f.uploaded_timestamp.unwrap_or(0));
            if let Some(newer) = same_title.or_else(|| current_main()) {
                let differs = match (&installed_version, newer.display_version()) {
                    (Some(a), Some(b)) => compare_versions(a, &b) == VersionRelation::Update,
                    _ => newer.file_id != file.file_id,
                };
                if differs {
                    return update_to(installed_version, newer);
                }
            }
        }

        return Decision {
            state: DecisionState::UpToDate,
            latest_version: installed_version.clone(),
            installed_version,
            latest_file_name: Some(file.name.clone()),
        };
    }

    let main = current_main();
    let latest_version = main.and_then(|f| f.display_version());
    match (&installed.version, &latest_version) {
        (Some(have), Some(latest)) => match compare_versions(have, latest) {
            VersionRelation::Update => update_to(installed.version.clone(), main.unwrap()),
            VersionRelation::Same | VersionRelation::InstalledNewer => Decision {
                state: DecisionState::UpToDate,
                installed_version: installed.version.clone(),
                latest_version,
                latest_file_name: main.map(|f| f.name.clone()),
            },
        },
        _ => Decision {
            state: DecisionState::Unknown,
            installed_version: installed.version.clone(),
            latest_version,
            latest_file_name: main.map(|f| f.name.clone()),
        },
    }
}

/// A previous check's decision for one mod, reused while Nexus's "updated
/// mods" list says nothing about that mod has changed since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CachedDecision {
    pub checked_at: i64,
    pub installed_key: String,
    pub decision: Decision,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Cache {
    #[serde(default)]
    pub nexus: HashMap<String, CachedDecision>,
}

pub async fn load_cache(base_path: &Path) -> Cache {
    match tokio::fs::read(base_path.join(CACHE_FILE)).await {
        Ok(data) => serde_json::from_slice(&data).unwrap_or_default(),
        Err(_) => Cache::default(),
    }
}

pub async fn save_cache(base_path: &Path, cache: &Cache) {
    match serde_json::to_vec_pretty(cache) {
        Ok(data) => {
            if let Err(e) = tokio::fs::write(base_path.join(CACHE_FILE), data).await {
                log::warn!("Couldn't save the update-check cache: {e}");
            }
        }
        Err(e) => log::warn!("Couldn't serialize the update-check cache: {e}"),
    }
}

/// A mod checked less than this long ago isn't checked again (like Mod
/// Organizer 2's 5-minute limit), so repeated clicks don't spend requests.
pub const RECHECK_AFTER_SECS: i64 = 5 * 60;
/// Mods not checked within this long (or never) get their own files
/// request: the "updated mods" list only covers the last month. 28 days
/// rather than 30 so the `1m` window always covers the gap.
pub const PER_MOD_AFTER_SECS: i64 = 28 * 86_400;
/// Slack when comparing a mod's last check with Nexus's
/// `latest_file_update` (clock differences, as in MO2).
pub const UPDATED_SLACK_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    pub fn as_param(self) -> &'static str {
        match self {
            Period::Day => "1d",
            Period::Week => "1w",
            Period::Month => "1m",
        }
    }
}

/// One installed Nexus mod as the planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanInput {
    pub mod_id: u64,
    /// When DDMM last checked this mod *for the file that's installed now*
    /// (`None`: never, or a different file was installed since).
    pub last_checked: Option<i64>,
}

/// What a check will ask Nexus, following Mod Organizer 2's
/// `checkAllForUpdate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Checked within [`RECHECK_AFTER_SECS`]: reuse the cached result, no
    /// request.
    pub recent: Vec<u64>,
    /// Never checked, or not within [`PER_MOD_AFTER_SECS`]: one
    /// `files.json` request each.
    pub per_mod: Vec<u64>,
    /// Everything else: covered by one `updated.json?period=` request with
    /// the smallest window reaching back to the oldest of their checks; only
    /// the mods it lists as changed since their check get a files request.
    pub via_updated: Vec<u64>,
    pub period: Option<Period>,
}

impl Plan {
    /// Every mod was checked moments ago: nothing to ask Nexus.
    pub fn all_recent(&self) -> bool {
        self.per_mod.is_empty() && self.via_updated.is_empty()
    }
}

pub fn plan(mods: &[PlanInput], now: i64) -> Plan {
    let mut out = Plan { recent: vec![], per_mod: vec![], via_updated: vec![], period: None };
    let mut earliest: Option<i64> = None;
    for m in mods {
        match m.last_checked {
            Some(t) if now - t < RECHECK_AFTER_SECS => out.recent.push(m.mod_id),
            Some(t) if now - t < PER_MOD_AFTER_SECS => {
                out.via_updated.push(m.mod_id);
                earliest = Some(earliest.map_or(t, |e: i64| e.min(t)));
            }
            _ => out.per_mod.push(m.mod_id),
        }
    }
    out.period = earliest.map(|t| {
        let age = now - t;
        if age < 86_400 - UPDATED_SLACK_SECS {
            Period::Day
        } else if age < 7 * 86_400 - UPDATED_SLACK_SECS {
            Period::Week
        } else {
            Period::Month
        }
    });
    out
}

/// Whether the "updated mods" list says `mod_id` got a new file since it
/// was last checked (only those get a files request).
pub fn changed_since(updated: &[UpdatedEntry], mod_id: u64, last_checked: i64) -> bool {
    updated
        .iter()
        .any(|u| u.mod_id == mod_id && u.latest_file_update > last_checked - UPDATED_SLACK_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: &str = include_str!("../../tests/fixtures/updates/nexus_files.json");
    const UPDATED: &str = include_str!("../../tests/fixtures/updates/nexus_updated_1w.json");
    const VALIDATE: &str = include_str!("../../tests/fixtures/updates/nexus_validate.json");

    fn files() -> ModFiles {
        serde_json::from_str(FILES).unwrap()
    }

    #[test]
    fn validate_response_parses_and_drops_the_echoed_key_and_email() {
        let user: ValidatedUser = serde_json::from_str(VALIDATE).unwrap();
        assert_eq!(user.name, "ExampleDiver");
        let debug = format!("{user:?}");
        assert!(!debug.contains("FIXTUREKEY"), "{debug}");
        assert!(!debug.contains("@"), "{debug}");
    }

    #[test]
    fn updated_list_parses() {
        let list: Vec<UpdatedEntry> = serde_json::from_str(UPDATED).unwrap();
        assert!(list.iter().any(|u| u.mod_id == 1234));
    }

    #[test]
    fn follows_the_file_update_chain_from_the_installed_file() {
        // Installed 1.0 (file 10), which was superseded by 1.1 (11), then 1.2 (12).
        let installed = Installed { file_id: Some(10), ..Default::default() };
        let d = decide(&installed, &files());
        assert_eq!(d.state, DecisionState::Update);
        assert_eq!(d.installed_version.as_deref(), Some("1.0"));
        assert_eq!(d.latest_version.as_deref(), Some("1.2"));
        assert_eq!(d.latest_file_name.as_deref(), Some("Better Stims"));
    }

    #[test]
    fn matches_the_installed_file_by_upload_time_from_its_archive_name() {
        let installed = Installed {
            version: Some("1.1".into()),
            uploaded_at: Some(1718100000),
            ..Default::default()
        };
        let d = decide(&installed, &files());
        assert_eq!(d.state, DecisionState::Update);
        assert_eq!(d.latest_version.as_deref(), Some("1.2"));
    }

    #[test]
    fn latest_main_file_is_up_to_date() {
        let installed = Installed { file_id: Some(12), ..Default::default() };
        let d = decide(&installed, &files());
        assert_eq!(d.state, DecisionState::UpToDate);
        assert_eq!(d.latest_version.as_deref(), Some("1.2"));
    }

    #[test]
    fn optional_files_are_tracked_separately_from_the_main_file() {
        // Optional file 20 (no-sound variant) is current and has no newer
        // link: up to date, even though the main file is at 1.2.
        let installed = Installed { file_id: Some(20), ..Default::default() };
        let d = decide(&installed, &files());
        assert_eq!(d.state, DecisionState::UpToDate);
        assert_eq!(d.latest_file_name.as_deref(), Some("Better Stims - No Sounds"));

        // Its old version (19), moved to OLD_VERSION without a link, is
        // matched to the current file with the same title.
        let installed = Installed { file_id: Some(19), ..Default::default() };
        let d = decide(&installed, &files());
        assert_eq!(d.state, DecisionState::Update);
        assert_eq!(d.latest_file_name.as_deref(), Some("Better Stims - No Sounds"));
        assert_eq!(d.latest_version.as_deref(), Some("1.2"));
    }

    #[test]
    fn unmatched_file_falls_back_to_version_vs_primary() {
        let d = decide(&Installed { version: Some("1.2".into()), ..Default::default() }, &files());
        assert_eq!(d.state, DecisionState::UpToDate);
        let d = decide(&Installed { version: Some("0.9".into()), ..Default::default() }, &files());
        assert_eq!(d.state, DecisionState::Update);
        assert_eq!(d.latest_version.as_deref(), Some("1.2"));
    }

    #[test]
    fn nothing_known_is_unknown_not_guessed() {
        let d = decide(&Installed::default(), &files());
        assert_eq!(d.state, DecisionState::Unknown);
        assert_eq!(d.latest_version.as_deref(), Some("1.2"));
    }

    #[test]
    fn a_cycle_in_file_updates_terminates() {
        let mut f = files();
        f.file_updates.push(FileUpdate { old_file_id: 12, new_file_id: 10, uploaded_timestamp: Some(1) });
        let d = decide(&Installed { file_id: Some(10), ..Default::default() }, &f);
        assert!(matches!(d.state, DecisionState::Update | DecisionState::UpToDate));
    }

    #[test]
    fn rate_limit_headers_are_read() {
        let mut h = HeaderMap::new();
        h.insert("x-rl-hourly-remaining", HeaderValue::from_static("495"));
        h.insert("x-rl-daily-remaining", HeaderValue::from_static("5"));
        let rl = RateLimit::from_headers(&h);
        assert_eq!(rl.hourly_remaining, Some(495));
        assert_eq!(rl.daily_remaining, Some(5));
        assert!(rl.is_low());
        assert!(!RateLimit::default().is_low());
    }

    const NOW: i64 = 2_000_000_000;
    const H: i64 = 3600;
    const D: i64 = 86_400;

    fn input(mod_id: u64, age: Option<i64>) -> PlanInput {
        PlanInput { mod_id, last_checked: age.map(|a| NOW - a) }
    }

    #[test]
    fn plan_uses_the_smallest_window_covering_the_oldest_check() {
        let p = plan(&[input(1, Some(2 * H)), input(2, Some(10 * H))], NOW);
        assert_eq!(p.period, Some(Period::Day));
        assert_eq!(p.via_updated, vec![1, 2]);
        assert!(p.per_mod.is_empty() && p.recent.is_empty());

        let p = plan(&[input(1, Some(2 * H)), input(2, Some(3 * D))], NOW);
        assert_eq!(p.period, Some(Period::Week));

        let p = plan(&[input(1, Some(2 * H)), input(2, Some(10 * D))], NOW);
        assert_eq!(p.period, Some(Period::Month));

        // Right at a boundary, step up (the window must reach past the check).
        assert_eq!(plan(&[input(1, Some(D - 30 * 60))], NOW).period, Some(Period::Week));
        assert_eq!(plan(&[input(1, Some(7 * D - 30 * 60))], NOW).period, Some(Period::Month));
    }

    #[test]
    fn plan_checks_old_and_never_checked_mods_individually() {
        let p = plan(&[input(1, None), input(2, Some(40 * D)), input(3, Some(2 * D))], NOW);
        assert_eq!(p.per_mod, vec![1, 2]);
        assert_eq!(p.via_updated, vec![3]);
        assert_eq!(p.period, Some(Period::Week));

        // Only old/never-checked mods: no updated list at all.
        let p = plan(&[input(1, None), input(2, Some(29 * D))], NOW);
        assert_eq!(p.per_mod, vec![1, 2]);
        assert_eq!(p.period, None);
    }

    #[test]
    fn plan_skips_mods_checked_moments_ago() {
        let p = plan(&[input(1, Some(60)), input(2, Some(4 * 60))], NOW);
        assert_eq!(p.recent, vec![1, 2]);
        assert!(p.all_recent());
        assert_eq!(p.period, None);

        let p = plan(&[input(1, Some(60)), input(2, Some(6 * 60))], NOW);
        assert_eq!(p.recent, vec![1]);
        assert_eq!(p.via_updated, vec![2]);
        assert!(!p.all_recent());
    }

    #[test]
    fn only_mods_the_updated_list_shows_as_changed_need_files() {
        let list: Vec<UpdatedEntry> = serde_json::from_str(UPDATED).unwrap();
        // Fixture: 1234 got a file at 1718200500, 88 at 1718150000.
        assert!(changed_since(&list, 1234, 1718100000), "new file after our check");
        assert!(!changed_since(&list, 1234, 1718300000), "our check is newer than its last file");
        assert!(changed_since(&list, 1234, 1718200500 + 1800), "within the 1 h slack");
        assert!(!changed_since(&list, 5678, 0), "not in the list at all");
    }

    /// Minimal one-shot HTTP server: answers every request with `body` and
    /// records the raw request text.
    async fn mock_server(status: &'static str, body: &'static str) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            if let Ok(Ok((mut sock, _))) =
                tokio::time::timeout(Duration::from_secs(5), listener.accept()).await
            {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                requests.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nx-rl-hourly-remaining: 499\r\nx-rl-daily-remaining: 19999\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
            requests
        });
        (format!("http://{addr}/v1/"), handle)
    }

    #[tokio::test]
    async fn sends_the_required_headers_and_reads_rate_limits() {
        let (base, server) = mock_server("200 OK", r#"{"user_id":1,"key":"K","name":"Tester","is_premium":false,"email":"a@b"}"#).await;
        let key = NexusApiKey::parse("HeaderTestKey").unwrap();
        let mut client = NexusClient::with_base(key.into(), &base).unwrap();
        let user = client.validate().await.unwrap();
        assert_eq!(user.name, "Tester");
        assert_eq!(client.rate.hourly_remaining, Some(499));

        let request = server.await.unwrap().join("\n").to_ascii_lowercase();
        assert!(request.starts_with("get /v1/users/validate.json "), "{request}");
        assert!(request.contains("apikey: headertestkey"));
        // Nothing blank, nothing borrowed from another application: our own
        // name, the real app version, and our own User-Agent.
        assert!(!APP_NAME.trim().is_empty() && !env!("CARGO_PKG_VERSION").trim().is_empty());
        assert!(request.contains("user-agent: democracydefendermodmanager/"));
        for line in request.lines() {
            if let Some((name, value)) = line.split_once(':') {
                assert!(!value.trim().is_empty(), "blank header {name}");
            }
        }
        assert!(request.contains("application-name: democracy defender mod manager"));
        assert!(request.contains(&format!("application-version: {}", env!("CARGO_PKG_VERSION"))));
    }

    #[tokio::test]
    async fn invalid_key_maps_to_invalid_key_error_without_leaking_it() {
        let (base, _server) = mock_server("401 Unauthorized", r#"{"message":"Please provide a valid API Key"}"#).await;
        let key = NexusApiKey::parse("LeakCheckKey").unwrap();
        let mut client = NexusClient::with_base(key.into(), &base).unwrap();
        let err = client.validate().await.unwrap_err();
        assert_eq!(err, NexusError::InvalidKey);
        assert!(!err.to_string().contains("LeakCheckKey"));
    }

    #[tokio::test]
    async fn redirects_are_never_followed() {
        let (base, _server) = mock_server("302 Found\r\nLocation: http://127.0.0.1:9/steal", "").await;
        let mut client = NexusClient::with_base(NexusApiKey::parse("K").unwrap().into(), &base).unwrap();
        let err = client.validate().await.unwrap_err();
        assert!(matches!(err, NexusError::Other(ref m) if m.contains("302")), "{err:?}");
    }

    #[tokio::test]
    async fn stops_before_requesting_when_quota_is_low() {
        let mut client = NexusClient::new(NexusApiKey::parse("K").unwrap().into()).unwrap();
        client.rate = RateLimit { hourly_remaining: Some(2), daily_remaining: Some(0) };
        assert_eq!(client.files(1).await.unwrap_err(), NexusError::RateLimited);
    }

    #[test]
    fn production_client_only_targets_the_nexus_api_over_https() {
        let client = NexusClient::new(NexusApiKey::parse("K").unwrap().into()).unwrap();
        assert_eq!(client.base.scheme(), "https");
        assert_eq!(client.base.host_str(), Some(API_HOST));
    }

    #[tokio::test]
    async fn signed_in_requests_carry_a_bearer_token_and_no_apikey() {
        let (base, server) = mock_server("200 OK", r#"{"user_id":1,"name":"Tester","is_premium":true}"#).await;
        let auth = NexusAuth::OAuth(Secret::new("BearerTokenXYZ"));
        let mut client = NexusClient::with_base(auth, &base).unwrap();
        client.validate().await.unwrap();
        let request = server.await.unwrap().join("\n");
        let lower = request.to_ascii_lowercase();
        assert!(request.contains("authorization: Bearer BearerTokenXYZ"), "{request}");
        assert!(!lower.contains("\napikey:"), "{request}");
        assert!(lower.contains("application-name: democracy defender mod manager"));
        assert!(lower.contains(&format!("application-version: {}", env!("CARGO_PKG_VERSION"))));
    }

    #[tokio::test]
    async fn rejected_sign_in_maps_to_invalid_key_without_leaking_the_token() {
        let (base, _server) = mock_server("401 Unauthorized", r#"{"message":"Token revoked"}"#).await;
        let mut client = NexusClient::with_base(NexusAuth::OAuth(Secret::new("LeakyBearer")), &base).unwrap();
        let err = client.validate().await.unwrap_err();
        assert_eq!(err, NexusError::InvalidKey);
        assert!(!format!("{err} {err:?}").contains("LeakyBearer"));
        assert!(!format!("{:?}", client.auth()).contains("LeakyBearer"));
    }
}
