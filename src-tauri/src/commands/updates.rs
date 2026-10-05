//! Mod update checks and one-click updates.
//!
//! **Update checks never run unless the user asks**: either by clicking
//! "Check for Updates", or by turning on "Check for mod updates when DDMM
//! starts" (and optionally "then every N hours while open") in Settings --
//! both off by default. See [`spawn_auto_check`].
//!
//! Sites checked (see `crate::providers`): AyakaMods (page JSON-LD),
//! GitHub (latest release), GameBanana (apiv11), ModWorkshop (public API),
//! and Nexus Mods -- only when the user has (optionally) signed in to Nexus
//! Mods or added a personal API key; without either, Nexus mods report
//! "optional: sign in or add a key to check".
//!
//! Updating: sites that serve the new file publicly (GitHub release
//! assets, GameBanana, ModWorkshop) can be updated in place with one click
//! ([`update_mod_direct`]). Everything else (AyakaMods, Nexus, anything
//! login-gated) goes through the user's browser -- the extension's "Update
//! with DDMM" button, or the Downloads-folder handoff.

use std::{path::Path, time::Duration};

use anyhow_tauri::{IntoTAResult, TAResult};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::{
    commands::{
        mods::{ensure_mods_loaded, install_update_from_archive, InstalledMod},
        settings::do_load_settings,
    },
    models::{manifest::Source, Mod},
    providers::{
        self, ayakamods, compare_versions, file_shape, gamebanana, github, modworkshop,
        nexus::{self, NexusAuth, NexusClient, NexusError},
        Pacer, UpdateFile, VersionRelation,
    },
    nexus_oauth::{self, ResolvedAuth},
    sources::{self, InstalledFile, SourceOrigin},
    AppState,
};

/// Minimum gap between two requests to the same keyless site.
const SAME_HOST_DELAY: Duration = Duration::from_secs(1);

/// Providers this manager knows how to check. Anything else is skipped
/// entirely (no entry produced).
const CHECKABLE_PROVIDERS: [&str; 5] = ["ayakamods", "github", "gamebanana", "modworkshop", "nexus"];
/// Providers whose new files DDMM may download itself (public, keyless).
const DIRECT_PROVIDERS: [&str; 3] = ["github", "gamebanana", "modworkshop"];

pub const CHECKED_EVENT: &str = "updates://checked";
pub const PROGRESS_EVENT: &str = "updates://progress";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateStatusEntry {
    pub guid: Uuid,
    pub provider: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    /// The newer file's title, for sites with several files per mod.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_file_name: Option<String>,
    /// When the mod's page was last updated (Unix seconds), for sites that
    /// say (AyakaMods). "Skip this version" records it, so a later update
    /// with the same version string isn't hidden too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_modified_at: Option<i64>,
    pub status: UpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    /// How this update would be installed (only set when one is available
    /// or skipped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<UpdateMethod>,
    /// Direct-download candidates (only for `Method: Direct`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<UpdateFile>,
    /// The candidate matching the installed file, when one clearly does --
    /// then no "which file?" question is needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preselected_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "Kind", rename_all = "PascalCase")]
pub enum UpdateState {
    UpToDate,
    UpdateAvailable,
    /// An update the user chose "Skip this version" for.
    Skipped,
    /// Not enough information to say either way (no installed version on
    /// record, or the site didn't say) -- never guessed.
    Unknown,
    /// A Nexus Mods source, and the user neither signed in to Nexus Mods
    /// nor added a personal API key (both optional).
    NeedsApiKey,
    /// A Nexus Mods source during an automatic check. Nexus's API
    /// Acceptable Use Policy only allows using a user's key for actions the
    /// user started, so automatic checks never call the Nexus API; the user
    /// checks Nexus mods by clicking "Check for Updates".
    NeedsManualCheck,
    /// Kept for compatibility; no longer produced.
    Unsupported,
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateMethod {
    /// DDMM downloads the new file itself and updates in place.
    Direct,
    /// Needs the user's browser (login-gated site, or no direct file).
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckTrigger {
    Manual,
    Startup,
    Scheduled,
}

impl CheckTrigger {
    /// Whether the user started this check themselves. Only such checks
    /// may use their Nexus API key (Nexus API Acceptable Use Policy).
    pub fn is_user_initiated(self) -> bool {
        matches!(self, CheckTrigger::Manual)
    }
}

/// Makes the Nexus API client for a check. Production always uses
/// [`NexusClient::new`] (fixed `https://api.nexusmods.com` base); tests
/// substitute a local mock to count requests.
type NexusClientFactory = dyn Fn(NexusAuth) -> anyhow::Result<NexusClient> + Send + Sync;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateCheckReport {
    pub trigger: CheckTrigger,
    /// Unix seconds.
    pub checked_at: i64,
    pub results: Vec<UpdateStatusEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nexus_rate_limit: Option<nexus::RateLimit>,
    /// All Nexus mods had been checked moments ago, so Nexus wasn't asked
    /// again (to save the user's API requests); results are the last ones.
    #[serde(default)]
    pub nexus_checked_recently: bool,
}

impl UpdateCheckReport {
    pub fn available_count(&self) -> usize {
        let mut guids: Vec<Uuid> = self
            .results
            .iter()
            .filter(|r| r.status == UpdateState::UpdateAvailable)
            .map(|r| r.guid)
            .collect();
        guids.sort();
        guids.dedup();
        guids.len()
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One (mod, source) pair to check.
#[derive(Debug, Clone)]
struct Target {
    guid: Uuid,
    provider: String,
    id: String,
    display_name: String,
    page_url: Option<String>,
    installed_version: Option<String>,
    installed_file: Option<InstalledFile>,
    /// "Skip this version" for this provider, if the user chose it.
    skipped: Option<sources::SkippedVersion>,
    /// When DDMM itself installed this mod from this site (Unix seconds;
    /// from the origin sidecar, only when it records this provider).
    installed_at: Option<i64>,
    /// The mod's folder (where its origin sidecar lives).
    directory: std::path::PathBuf,
}

/// Every checkable (provider, id) source of every installed mod, once each.
/// The installed version prefers what DDMM itself recorded at install time
/// (it knows what it actually installed) over a manifest-declared one.
async fn collect_targets(mods: &[Mod]) -> Vec<Target> {
    let mut targets = Vec::new();
    for m in mods {
        let sidecar = sources::load_origin_sidecar(&m.directory).await;
        let mut seen: Vec<(String, String)> = Vec::new();
        for source in &m.sources {
            let provider = source.provider.trim().to_ascii_lowercase();
            if !CHECKABLE_PROVIDERS.contains(&provider.as_str()) {
                continue;
            }
            let Some(id) = sources::resolved_source_id(source) else { continue };
            if seen.iter().any(|(p, i)| p == &provider && i == &id) {
                continue;
            }
            seen.push((provider.clone(), id.clone()));

            let same = |s: &&sources::ResolvedSource| {
                s.provider.eq_ignore_ascii_case(&provider) && sources::resolved_source_id(s).as_deref() == Some(id.as_str())
            };
            let installed_version = m
                .sources
                .iter()
                .filter(same)
                .filter(|s| s.origin == SourceOrigin::Install)
                .find_map(|s| s.version.clone())
                .or_else(|| m.sources.iter().filter(same).find_map(|s| s.version.clone()));

            let installed_file = sidecar
                .as_ref()
                .and_then(|s| s.installed_files.iter().find(|f| f.provider.eq_ignore_ascii_case(&provider)).cloned());
            let skipped = sidecar
                .as_ref()
                .and_then(|s| s.skipped_versions.iter().find(|v| v.provider.eq_ignore_ascii_case(&provider)).cloned());
            let installed_at = sidecar
                .as_ref()
                .filter(|s| s.sources.iter().any(|src| src.provider.eq_ignore_ascii_case(&provider)))
                .map(|s| s.installed_at as i64)
                .filter(|t| *t > 0);

            targets.push(Target {
                guid: m.guid(),
                provider,
                id,
                display_name: source.display_name.clone(),
                page_url: source.page_url.clone(),
                installed_version,
                installed_file,
                skipped,
                installed_at,
                directory: m.directory.clone(),
            });
        }
    }
    targets
}

fn status_for(installed: Option<&str>, latest: Option<&str>) -> UpdateState {
    match (installed, latest) {
        (Some(i), Some(l)) if !l.trim().is_empty() => match compare_versions(i, l) {
            VersionRelation::Update => UpdateState::UpdateAvailable,
            VersionRelation::Same | VersionRelation::InstalledNewer => UpdateState::UpToDate,
        },
        _ => UpdateState::Unknown,
    }
}

/// How long after DDMM installed an AyakaMods mod its page may still show
/// a later "last updated" time without that counting as an update -- only
/// used when the page's own time wasn't recorded at install. The site's
/// last-update time can trail the file's upload by a minute or two (the
/// update notes are posted after the file).
const AYAKAMODS_INSTALL_GRACE_SECS: i64 = 10 * 60;

/// A difference between this PC's clock and the site's smaller than this
/// is just network delay and rounding, not a wrong clock.
const CLOCK_SKEW_MIN_SECS: i64 = 60;

/// AyakaMods' update decision.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AyakaDecision {
    state: UpdateState,
    /// The page time to record as this mod's baseline: set when no page
    /// time was recorded yet and the comparison with the install time
    /// showed the installed file *is* the page's current one. Later checks
    /// then compare page time with page time, whatever this PC's clock
    /// says. (Never set for an update: that must stay flagged until the
    /// user installs it.)
    record_baseline: Option<i64>,
}

/// AyakaMods' update decision. Its version strings are often just the date
/// of the update (`2026-10-01`), so two updates on one day look the same;
/// the page's last-update time (`dateModified`) tells them apart. An
/// update is available when the page was updated after what's installed
/// (compared with the page time recorded at install, or else with when
/// DDMM installed it), or when the version differs. Unknown only when
/// neither can be compared.
///
/// `clock_offset` is how far the site's clock is ahead of this PC's (from
/// the response's `Date` header): the install time was taken from this
/// PC's clock, so it is moved by that much before comparing it with the
/// site's time.
fn ayakamods_status(
    installed_version: Option<&str>,
    recorded_modified: Option<i64>,
    installed_at: Option<i64>,
    clock_offset: i64,
    meta: &ayakamods::AyakaModsMetadata,
) -> AyakaDecision {
    let clock_offset = if clock_offset.abs() < CLOCK_SKEW_MIN_SECS { 0 } else { clock_offset };
    let mut record_baseline = None;
    let newer_on_page = match (meta.modified_at, recorded_modified, installed_at) {
        (Some(page), Some(recorded), _) => Some(page > recorded),
        (Some(page), None, Some(installed)) => {
            let newer = page > installed + clock_offset + AYAKAMODS_INSTALL_GRACE_SECS;
            if !newer {
                record_baseline = Some(page);
            }
            Some(newer)
        }
        _ => None,
    };
    let by_version = status_for(installed_version, meta.latest_version.as_deref());
    let state = match (newer_on_page, by_version) {
        (Some(true), _) | (_, UpdateState::UpdateAvailable) => UpdateState::UpdateAvailable,
        (Some(false), _) => UpdateState::UpToDate,
        (None, state) => state,
    };
    if state != UpdateState::UpToDate {
        record_baseline = None;
    }
    AyakaDecision { state, record_baseline }
}

/// Whether two version strings name the same version, the way update
/// checks compare them (`v1.2` and `1.2` do; so do `1-2` and `1.2`) -- used
/// for "Skip this version", which must not come back because a site
/// started (or stopped) writing a leading `v`.
fn same_version(a: &str, b: &str) -> bool {
    compare_versions(a, b) == VersionRelation::Same
}

/// Whether "Skip this version" `skip` covers the update `latest` whose page
/// was last updated at `page_time` (AyakaMods only). The version must match;
/// and when both the skip and the page have a time, the page must not have
/// been updated since the skip -- AyakaMods versions are often a date or
/// never change, so a newer page time is a new update even with the same
/// version string. Skips saved before times were recorded, and sites
/// without page times, go by the version alone. A skip of an update whose
/// page gave no version (empty `version`) goes by the page time alone.
fn skip_covers(skip: &sources::SkippedVersion, latest: Option<&str>, page_time: Option<i64>) -> bool {
    if skip.version.is_empty() {
        return matches!((page_time, skip.modified_at), (Some(page), Some(skipped_at)) if page <= skipped_at);
    }
    let Some(latest) = latest else { return false };
    if !same_version(&skip.version, latest) {
        return false;
    }
    match (page_time, skip.modified_at) {
        (Some(page), Some(skipped_at)) => page <= skipped_at,
        _ => true,
    }
}

fn entry(t: &Target, status: UpdateState, latest: Option<String>) -> UpdateStatusEntry {
    UpdateStatusEntry {
        guid: t.guid,
        provider: t.provider.clone(),
        display_name: t.display_name.clone(),
        source_id: Some(t.id.clone()),
        installed_version: t.installed_version.clone(),
        latest_version: latest,
        latest_file_name: None,
        latest_modified_at: None,
        status,
        page_url: t.page_url.clone(),
        method: None,
        files: Vec::new(),
        preselected_file: None,
    }
}

fn error_entry(t: &Target, e: impl std::fmt::Display) -> UpdateStatusEntry {
    entry(t, UpdateState::Error { message: e.to_string() }, None)
}

/// The candidate that clearly corresponds to the installed file: the only
/// one, or the one with the same label/name shape as what was installed.
pub fn preselect(files: &[UpdateFile], installed: Option<&InstalledFile>) -> Option<String> {
    if files.len() == 1 {
        return Some(files[0].id.clone());
    }
    let installed = installed?;
    if let Some(label) = installed.label.as_deref().filter(|l| !l.trim().is_empty()) {
        let matches: Vec<_> = files
            .iter()
            .filter(|f| f.label.as_deref().is_some_and(|fl| fl.trim().eq_ignore_ascii_case(label.trim())))
            .collect();
        if matches.len() == 1 {
            return Some(matches[0].id.clone());
        }
    }
    if let Some(name) = installed.file_name.as_deref() {
        let shape = file_shape(name);
        if !shape.is_empty() {
            let matches: Vec<_> = files.iter().filter(|f| file_shape(&f.name) == shape).collect();
            if matches.len() == 1 {
                return Some(matches[0].id.clone());
            }
        }
    }
    None
}

async fn check_keyless(client: &reqwest::Client, pacer: &mut Pacer, t: &Target) -> UpdateStatusEntry {
    match t.provider.as_str() {
        // AyakaMods is checked in `run_check_with` (it may stop asking
        // the site partway through a check).
        "github" => {
            pacer.wait(github::API_HOST, SAME_HOST_DELAY).await;
            match github::latest_release(client, &t.id).await {
                Ok(Some(release)) => {
                    let tag = release.tag_name.clone().filter(|t| !t.is_empty());
                    let status = status_for(t.installed_version.as_deref(), tag.as_deref());
                    let mut e = entry(t, status, tag);
                    e.files = github::candidates(&release);
                    e.method = Some(if e.files.is_empty() { UpdateMethod::Browser } else { UpdateMethod::Direct });
                    if e.method == Some(UpdateMethod::Browser) {
                        // Point the browser at the release itself.
                        e.page_url = release.html_url.clone().or(e.page_url);
                    }
                    e
                }
                Ok(None) => entry(t, UpdateState::Unknown, None),
                Err(err) => error_entry(t, err),
            }
        }
        "gamebanana" => {
            pacer.wait(gamebanana::HOST, SAME_HOST_DELAY).await;
            match gamebanana::fetch_mod(client, &t.id).await {
                Ok(m) if !gamebanana::is_available(&m) => {
                    error_entry(t, "this mod is private, withheld or deleted on GameBanana")
                }
                Ok(m) => {
                    let latest = gamebanana::latest_version(&m);
                    let status = status_for(t.installed_version.as_deref(), latest.as_deref());
                    let mut e = entry(t, status, latest);
                    e.files = gamebanana::candidates(&m);
                    e.method = Some(if e.files.is_empty() { UpdateMethod::Browser } else { UpdateMethod::Direct });
                    e
                }
                Err(err) => error_entry(t, err),
            }
        }
        "modworkshop" => {
            pacer.wait(modworkshop::HOST, SAME_HOST_DELAY).await;
            match modworkshop::fetch_mod(client, &t.id).await {
                Ok(m) if !modworkshop::is_available(&m) => {
                    error_entry(t, "this mod is private or suspended on ModWorkshop")
                }
                Ok(m) => {
                    let latest = modworkshop::latest_version(&m);
                    let status = status_for(t.installed_version.as_deref(), latest.as_deref());
                    let mut all_files = None;
                    // The file list costs a second request; only worth it
                    // when there's an update and more than one file.
                    if status == UpdateState::UpdateAvailable
                        && modworkshop::has_direct_files(&m)
                        && m.files_count.unwrap_or(1) > 1
                    {
                        pacer.wait(modworkshop::HOST, SAME_HOST_DELAY).await;
                        match modworkshop::fetch_files(client, &t.id).await {
                            Ok(files) => all_files = Some(files),
                            Err(err) => log::warn!("ModWorkshop file list for mod {}: {err}", t.id),
                        }
                    }
                    let mut e = entry(t, status, latest);
                    e.files = modworkshop::candidates(&m, all_files.as_deref());
                    e.method = Some(if e.files.is_empty() { UpdateMethod::Browser } else { UpdateMethod::Direct });
                    e
                }
                Err(err) => error_entry(t, err),
            }
        }
        _ => entry(t, UpdateState::Unknown, None),
    }
}

/// The entry for an AyakaMods target, and the page time to record as its
/// baseline, if any (see [`AyakaDecision::record_baseline`]).
fn ayakamods_entry(t: &Target, meta: &ayakamods::AyakaModsMetadata) -> (UpdateStatusEntry, Option<i64>) {
    let recorded_modified = t.installed_file.as_ref().and_then(|f| f.uploaded_at);
    let clock_offset = meta.server_time.map(|server| server - now_unix()).unwrap_or(0);
    let decision =
        ayakamods_status(t.installed_version.as_deref(), recorded_modified, t.installed_at, clock_offset, meta);
    let mut e = entry(t, decision.state, meta.latest_version.clone());
    // Same version string, newer update (two updates on one day): show
    // when the page was updated, so "2026-10-01 -> 2026-10-01" makes sense.
    let same_label = match (t.installed_version.as_deref(), meta.latest_version.as_deref()) {
        (Some(i), Some(l)) => same_version(i, l),
        _ => true,
    };
    if same_label {
        e.latest_file_name = meta.modified_at.map(providers::format_timestamp);
    }
    e.latest_modified_at = meta.modified_at;
    e.method = Some(UpdateMethod::Browser);
    (e, decision.record_baseline)
}

/// Record `page_time` as the AyakaMods page time of what's installed in
/// `t`'s mod, unless the mod was reinstalled or updated in the meantime
/// (its install time changed) or a page time is already recorded.
async fn record_ayakamods_baseline(t: &Target, page_time: i64) -> anyhow::Result<()> {
    let Some(mut sidecar) = sources::load_origin_sidecar(&t.directory).await else { return Ok(()) };
    if Some(sidecar.installed_at as i64) != t.installed_at {
        return Ok(());
    }
    match sidecar.installed_files.iter_mut().find(|f| f.provider.eq_ignore_ascii_case("ayakamods")) {
        Some(f) if f.uploaded_at.is_some() => return Ok(()),
        Some(f) => f.uploaded_at = Some(page_time),
        None => sidecar.installed_files.push(InstalledFile {
            provider: "ayakamods".into(),
            uploaded_at: Some(page_time),
            ..Default::default()
        }),
    }
    sources::save_origin_sidecar(&t.directory, &sidecar).await
}

fn nexus_installed(t: &Target) -> nexus::Installed {
    let file = t.installed_file.as_ref();
    nexus::Installed {
        version: t.installed_version.clone(),
        file_id: file.and_then(|f| f.file_id.as_deref()).and_then(|i| i.parse().ok()),
        uploaded_at: file.and_then(|f| f.uploaded_at),
        file_name: file.and_then(|f| f.file_name.clone()),
    }
}

fn nexus_entry(t: &Target, d: &nexus::Decision) -> UpdateStatusEntry {
    let status = match d.state {
        nexus::DecisionState::UpToDate => UpdateState::UpToDate,
        nexus::DecisionState::Update => UpdateState::UpdateAvailable,
        nexus::DecisionState::Unknown => UpdateState::Unknown,
    };
    let mut e = entry(t, status, d.latest_version.clone());
    e.installed_version = d.installed_version.clone().or(e.installed_version);
    e.latest_file_name = d.latest_file_name.clone();
    e.method = Some(UpdateMethod::Browser);
    // Land on the Files tab, where the user clicks Nexus's own download.
    e.page_url = Some(format!("https://www.nexusmods.com/{}/mods/{}?tab=files", nexus::GAME_DOMAIN, t.id));
    e
}

/// The Nexus part of a user-initiated check.
struct NexusOutcome {
    entries: Vec<UpdateStatusEntry>,
    rate: Option<nexus::RateLimit>,
    /// Every Nexus mod had been checked moments ago, so nothing was asked.
    all_recent: bool,
}

/// Check every installed Nexus mod the way Mod Organizer 2 does, to spend
/// as few of the user's API requests as possible (see [`nexus::plan`]):
/// mods checked in the last few minutes are skipped; mods never checked, or
/// not within a month, get one `files.json` request each; the rest are
/// covered by a single `updated.json?period=1d|1w|1m` request, and only the
/// mods it lists as changed since their last check get a `files.json`
/// request. Returns entries in `targets` order.
async fn check_nexus(base_path: &Path, targets: &[&Target], make_client: &NexusClientFactory) -> NexusOutcome {
    // The one place that decides how Nexus requests authenticate: signed
    // in (refreshed if about to expire) > personal API key > nothing.
    let auth = match nexus_oauth::resolve_auth(base_path).await {
        ResolvedAuth::Auth(auth) => auth,
        ResolvedAuth::NoCredentials => {
            return NexusOutcome {
                entries: targets.iter().map(|t| entry(t, UpdateState::NeedsApiKey, None)).collect(),
                rate: None,
                all_recent: false,
            }
        }
        ResolvedAuth::SignedOut(message) | ResolvedAuth::Unavailable(message) => {
            return NexusOutcome { entries: targets.iter().map(|t| error_entry(t, &message)).collect(), rate: None, all_recent: false }
        }
    };
    let signed_in = auth.is_oauth();
    // A 401/403 on a signed-in request means the sign-in was revoked: say
    // so (the sign-in is removed below) instead of talking about a key.
    let describe = |e: &NexusError| -> String {
        if signed_in && *e == NexusError::InvalidKey {
            nexus_oauth::SIGNED_OUT_MESSAGE.to_string()
        } else {
            e.to_string()
        }
    };
    let mut client = match make_client(auth) {
        Ok(c) => c,
        Err(e) => {
            return NexusOutcome { entries: targets.iter().map(|t| error_entry(t, &e)).collect(), rate: None, all_recent: false }
        }
    };

    let now = now_unix();
    let mut cache = nexus::load_cache(base_path).await;
    let mut pacer = Pacer::default();

    // What we know about each target: its id, installed file, the cache
    // entry for exactly that installed file, and when it was last checked.
    // Keyed by mod id *and* installed file: several files of one Nexus page
    // installed as separate mods (a main file and an optional one) each
    // keep their own entry instead of overwriting each other's, which would
    // cost a files request per mod on every check.
    type TargetInfo = (u64, nexus::Installed, String, Option<i64>);
    let info: Vec<Option<TargetInfo>> = targets
        .iter()
        .map(|t| {
            let mod_id = t.id.parse::<u64>().ok()?;
            let installed = nexus_installed(t);
            let installed_key = installed.cache_key();
            let key = nexus::cache_entry_key(&t.id, &installed_key);
            // An entry from before the cache was keyed per file (keyed by
            // mod id alone) still counts, but only for the very file it
            // was made for; it moves to the new key below.
            if !cache.nexus.contains_key(&key) {
                if let Some(old) = cache.nexus.get(&t.id).filter(|c| c.installed_key == installed_key).cloned() {
                    cache.nexus.insert(key.clone(), old);
                }
            }
            let last = cache.nexus.get(&key).filter(|c| c.installed_key == installed_key).map(|c| c.checked_at);
            Some((mod_id, installed, key, last))
        })
        .collect();

    let inputs: Vec<nexus::PlanInput> = info
        .iter()
        .flatten()
        .map(|(mod_id, _, _, last)| nexus::PlanInput { mod_id: *mod_id, last_checked: *last })
        .collect();
    let plan = nexus::plan(&inputs, now);
    let mut need_files: std::collections::HashSet<u64> = plan.per_mod.iter().copied().collect();
    let mut list_error: Option<String> = None;

    if let Some(period) = plan.period {
        pacer.wait(nexus::API_HOST, nexus::REQUEST_GAP).await;
        match client.updated(period).await {
            Ok(list) => {
                for (mod_id, _, key, last) in info.iter().flatten() {
                    if !plan.via_updated.contains(mod_id) {
                        continue;
                    }
                    let last = last.unwrap_or(0);
                    if nexus::changed_since(&list, *mod_id, last) {
                        need_files.insert(*mod_id);
                    } else if let Some(c) = cache.nexus.get_mut(key) {
                        // Nothing new since the last check: that result
                        // still holds, as of now.
                        c.checked_at = now;
                    }
                }
            }
            Err(e @ NexusError::InvalidKey) => {
                if signed_in {
                    nexus_oauth::handle_rejected_sign_in(base_path).await;
                }
                return NexusOutcome {
                    entries: targets.iter().map(|t| error_entry(t, describe(&e))).collect(),
                    rate: Some(client.rate),
                    all_recent: false,
                }
            }
            Err(e) => {
                log::warn!("Nexus updated-mods list unavailable: {e}");
                list_error = Some(e.to_string());
            }
        }
    }

    let mut results = Vec::with_capacity(targets.len());
    let mut fetched: std::collections::HashMap<u64, Result<nexus::ModFiles, NexusError>> = Default::default();
    let mut stop: Option<NexusError> = None;
    let mut calls = 0usize;
    for (t, i) in targets.iter().zip(&info) {
        let Some((mod_id, installed, key, _)) = i else {
            results.push(entry(t, UpdateState::Unknown, None));
            continue;
        };

        if !need_files.contains(mod_id) {
            if plan.via_updated.contains(mod_id) {
                if let Some(message) = &list_error {
                    results.push(error_entry(t, message));
                    continue;
                }
            }
            match cache.nexus.get(key) {
                Some(cached) => results.push(nexus_entry(t, &cached.decision)),
                None => results.push(entry(t, UpdateState::Unknown, None)),
            }
            continue;
        }

        if let Some(e) = &stop {
            results.push(error_entry(t, describe(e)));
            continue;
        }
        if !fetched.contains_key(mod_id) {
            pacer.wait(nexus::API_HOST, nexus::REQUEST_GAP).await;
            calls += 1;
            fetched.insert(*mod_id, client.files(*mod_id).await);
        }
        match &fetched[mod_id] {
            Ok(files) => {
                let decision = nexus::decide(installed, files);
                cache.nexus.insert(
                    key.clone(),
                    nexus::CachedDecision {
                        checked_at: now,
                        installed_key: installed.cache_key(),
                        decision: decision.clone(),
                    },
                );
                results.push(nexus_entry(t, &decision));
            }
            Err(e @ (NexusError::InvalidKey | NexusError::RateLimited)) => {
                results.push(error_entry(t, describe(e)));
                stop = Some(e.clone());
            }
            Err(e) => results.push(error_entry(t, e)),
        }
    }
    if signed_in && stop == Some(NexusError::InvalidKey) {
        nexus_oauth::handle_rejected_sign_in(base_path).await;
    }

    // Keep only entries for what's installed now: this drops legacy
    // (mod-id-only) keys once migrated, and entries for files that have
    // since been updated or removed, so the cache can't grow forever.
    let current: std::collections::HashSet<&String> = info.iter().flatten().map(|(_, _, key, _)| key).collect();
    cache.nexus.retain(|k, _| current.contains(k));
    nexus::save_cache(base_path, &cache).await;
    log::info!(
        "Nexus update check: {} mod(s): {} checked recently (skipped), {} individually, {} via updated list ({}); {} files request(s); quota left: hourly {:?}, daily {:?}",
        targets.len(),
        plan.recent.len(),
        plan.per_mod.len(),
        plan.via_updated.len(),
        plan.period.map(|p| p.as_param()).unwrap_or("not needed"),
        calls,
        client.rate.hourly_remaining,
        client.rate.daily_remaining
    );
    NexusOutcome { entries: results, rate: Some(client.rate), all_recent: plan.all_recent() && !inputs.is_empty() }
}

/// Run one full check. Serialized: a second request waits for the first.
pub async fn run_check(state: &AppState, trigger: CheckTrigger) -> anyhow::Result<UpdateCheckReport> {
    run_check_with(state, trigger, &NexusClient::new).await
}

async fn run_check_with(
    state: &AppState,
    trigger: CheckTrigger,
    make_nexus_client: &NexusClientFactory,
) -> anyhow::Result<UpdateCheckReport> {
    let _data_op = state.data_op()?;
    let _guard = state.update_check_lock.lock().await;
    log::info!("Checking for mod updates ({trigger:?})...");

    let mods = {
        let mut guard = state.mods.lock().await;
        ensure_mods_loaded(&mut guard, &state.base_path)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?
            .clone()
    };
    let targets = collect_targets(&mods).await;

    let client = providers::build_client()?;
    let mut pacer = Pacer::default();
    let mut by_index: Vec<Option<UpdateStatusEntry>> = vec![None; targets.len()];

    // Once AyakaMods turns DDMM away (a Cloudflare challenge, rate
    // limiting, or two refusals in a row), the rest of its mods aren't
    // asked again in this check.
    let mut ayakamods_refusing: Option<String> = None;
    let mut ayakamods_403s = 0;
    let mut ayakamods_baselines: Vec<(usize, i64)> = Vec::new();
    for (i, t) in targets.iter().enumerate().filter(|(_, t)| t.provider != "nexus") {
        if t.provider == "ayakamods" {
            if let Some(message) = &ayakamods_refusing {
                by_index[i] = Some(error_entry(t, message));
                continue;
            }
            let e = match ayakamods::fetch_ayakamods_metadata(&client, &t.id).await {
                Ok(Some(meta)) => {
                    let (e, baseline) = ayakamods_entry(t, &meta);
                    if let Some(page_time) = baseline {
                        ayakamods_baselines.push((i, page_time));
                    }
                    e
                }
                Ok(None) => {
                    log::warn!("Update check: AyakaMods mod {} has no version information on its page.", t.id);
                    entry(t, UpdateState::Unknown, None)
                }
                Err(err) => {
                    ayakamods_403s = if err == ayakamods::AyakaError::Forbidden { ayakamods_403s + 1 } else { 0 };
                    if err.site_is_refusing() || ayakamods_403s >= 2 {
                        ayakamods_refusing = Some(err.to_string());
                    }
                    error_entry(t, err)
                }
            };
            if !matches!(e.status, UpdateState::Error { .. }) {
                ayakamods_403s = 0;
            }
            by_index[i] = Some(e);
            continue;
        }
        by_index[i] = Some(check_keyless(&client, &mut pacer, t).await);
    }

    let nexus_targets: Vec<(usize, &Target)> = targets.iter().enumerate().filter(|(_, t)| t.provider == "nexus").collect();
    let mut nexus_rate = None;
    let mut nexus_all_recent = false;
    if !nexus_targets.is_empty() && trigger.is_user_initiated() {
        let refs: Vec<&Target> = nexus_targets.iter().map(|(_, t)| *t).collect();
        let outcome = check_nexus(&state.base_path, &refs, make_nexus_client).await;
        nexus_rate = outcome.rate;
        nexus_all_recent = outcome.all_recent;
        for ((i, _), e) in nexus_targets.iter().zip(outcome.entries) {
            by_index[*i] = Some(e);
        }
    } else if !nexus_targets.is_empty() {
        // Automatic check: no Nexus API calls at all (the user didn't start
        // it). Keep what a check the user ran earlier this session found;
        // otherwise say how to check. Reading whether a key exists is local.
        let has_key = nexus_oauth::has_credentials(&state.base_path).await;
        let previous = state.last_update_report.lock().await.clone();
        for (i, t) in &nexus_targets {
            let carried = previous.as_ref().and_then(|r| {
                r.results
                    .iter()
                    .find(|e| e.guid == t.guid && e.provider == "nexus" && e.status != UpdateState::NeedsManualCheck)
                    .cloned()
            });
            by_index[*i] = Some(match carried {
                Some(e) if has_key => e,
                _ if has_key => entry(t, UpdateState::NeedsManualCheck, None),
                _ => entry(t, UpdateState::NeedsApiKey, None),
            });
        }
        log::info!("Automatic check: skipped {} Nexus mod(s) (Nexus is only checked when you click Check for Updates).", nexus_targets.len());
    }

    let mut results = Vec::with_capacity(targets.len());
    // Skips saved without a page time that this check found the page time
    // for: (target, page time), recorded below with the mod list locked.
    let mut legacy_skips: Vec<(usize, i64)> = Vec::new();
    for (i, (t, e)) in targets.iter().zip(by_index).enumerate() {
        let mut e = e.unwrap_or_else(|| entry(t, UpdateState::Unknown, None));
        if e.method == Some(UpdateMethod::Direct) {
            e.preselected_file = preselect(&e.files, t.installed_file.as_ref());
        }
        if e.status == UpdateState::UpdateAvailable {
            if let Some(skip) = &t.skipped {
                if skip_covers(skip, e.latest_version.as_deref(), e.latest_modified_at) {
                    e.status = UpdateState::Skipped;
                    // Stamp an old skip with the time of the update it
                    // covers, so the page's next update shows again.
                    if let (None, Some(page)) = (skip.modified_at, e.latest_modified_at) {
                        legacy_skips.push((i, page));
                    }
                }
            }
        } else {
            e.method = None;
            e.files.clear();
            e.preselected_file = None;
        }
        results.push(e);
    }

    let mut report = UpdateCheckReport {
        trigger,
        checked_at: now_unix(),
        results,
        nexus_rate_limit: nexus_rate,
        nexus_checked_recently: nexus_all_recent,
    };
    // A mod deleted while the check waited on the network must not get its
    // update status back. The mod list stays locked until the report is
    // stored (the same order `delete_mod` locks them in), so a delete can't
    // slip in between.
    let listed = state.mods.lock().await;
    if let Some(listed) = listed.as_ref() {
        report.results.retain(|e| listed.iter().any(|m| m.guid() == e.guid));
        // Installs write the sidecar while holding the mod list, so this
        // can't interleave with one.
        for (i, page_time) in ayakamods_baselines {
            let t = &targets[i];
            if listed.iter().any(|m| m.guid() == t.guid) {
                if let Err(e) = record_ayakamods_baseline(t, page_time).await {
                    log::warn!("Couldn't record the AyakaMods page time of mod {}: {e}", t.id);
                }
            }
        }
        for (i, page_time) in legacy_skips {
            let t = &targets[i];
            let Some(skip) = t.skipped.as_ref() else { continue };
            if listed.iter().any(|m| m.guid() == t.guid) {
                if let Err(e) = sources::stamp_legacy_skip(&t.directory, &t.provider, &skip.version, page_time).await {
                    log::warn!("Couldn't record the page time of the skipped update of {} mod {}: {e}", t.provider, t.id);
                }
            }
        }
    }
    let mut failed = 0;
    for e in &report.results {
        if let UpdateState::Error { message } = &e.status {
            failed += 1;
            log::warn!(
                "Update check failed for {} mod {} (DDMM mod {{{}}}): {message}",
                e.provider,
                e.source_id.as_deref().unwrap_or("?"),
                e.guid
            );
        }
    }
    log::info!(
        "Update check done: {} source(s) checked, {} mod(s) with updates, {} source(s) couldn't be checked.",
        report.results.len(),
        report.available_count(),
        failed
    );
    *state.last_update_report.lock().await = Some(report.clone());
    drop(listed);
    *state.last_update_check.lock().await = Some(tokio::time::Instant::now());
    Ok(report)
}

/// "Check for Updates" button.
#[tauri::command]
pub async fn check_updates(state: State<'_, AppState>) -> TAResult<UpdateCheckReport> {
    run_check(&state, CheckTrigger::Manual).await.into_ta_result()
}

/// The last check's results (from this session), so the Mods page can show
/// badges for a check that ran before it was mounted (e.g. at startup).
#[tauri::command]
pub async fn get_last_update_report(state: State<'_, AppState>) -> TAResult<Option<UpdateCheckReport>> {
    Ok(state.last_update_report.lock().await.clone())
}

/// "Skip this version" (`version` and/or `modified_at` set) / "Stop
/// skipping" (both `None`). `modified_at` is the skipped update's page time
/// ([`UpdateStatusEntry::latest_modified_at`]), for sites that have one;
/// when not given, it is taken from the last check's matching entry.
#[tauri::command]
pub async fn skip_update_version(
    state: State<'_, AppState>,
    guid: Uuid,
    provider: String,
    version: Option<String>,
    modified_at: Option<i64>,
) -> TAResult<()> {
    set_skip(&state, guid, provider, version, modified_at).await.into_ta_result()
}

async fn set_skip(
    state: &AppState,
    guid: Uuid,
    provider: String,
    version: Option<String>,
    modified_at: Option<i64>,
) -> anyhow::Result<()> {
    let _data_op = state.data_op()?;
    // The sidecar is read and rewritten with the mod list locked, like every
    // other sidecar write an update check or update can make (then the
    // report, in the same order as a check locks them).
    let guard = state.mods.lock().await;
    let dir = guard
        .as_ref()
        .and_then(|mods| mods.iter().find(|m| m.guid() == guid))
        .map(|m| m.directory.clone())
        .ok_or_else(|| anyhow::anyhow!("mod {{{guid}}} not found"))?;
    let mut report = state.last_update_report.lock().await;
    let modified_at = match (&version, modified_at) {
        (Some(v), None) => report.as_ref().and_then(|r| {
            r.results
                .iter()
                .filter(|e| e.guid == guid && e.provider.eq_ignore_ascii_case(&provider))
                .find(|e| e.latest_version.as_deref().is_some_and(|l| same_version(v, l)))
                .and_then(|e| e.latest_modified_at)
        }),
        (_, t) => t,
    };
    // No version but a page time: skip that update by its time alone (an
    // AyakaMods page without a version). Neither: stop skipping.
    let skip = match (version, modified_at) {
        (Some(version), _) => Some(sources::SkippedVersion { provider: provider.clone(), version, modified_at }),
        (None, Some(t)) => Some(sources::SkippedVersion { provider: provider.clone(), version: String::new(), modified_at: Some(t) }),
        (None, None) => None,
    };
    sources::set_skipped_version(&dir, &provider, skip.as_ref().map(|s| s.version.as_str()), modified_at).await?;
    drop(guard);

    if let Some(report) = report.as_mut() {
        for e in report.results.iter_mut().filter(|e| e.guid == guid && e.provider.eq_ignore_ascii_case(&provider)) {
            match (&skip, &e.status) {
                (Some(skip), UpdateState::UpdateAvailable)
                    if skip_covers(skip, e.latest_version.as_deref(), e.latest_modified_at) =>
                {
                    e.status = UpdateState::Skipped
                }
                (None, UpdateState::Skipped) => e.status = UpdateState::UpdateAvailable,
                _ => {}
            }
        }
    }
    Ok(())
}

/// After any in-place update (direct, bridge or handoff): mark that mod's
/// entries in the last report as up to date, so its badge goes away.
pub async fn mark_mod_updated(state: &AppState, old_guid: Uuid, new_guid: Uuid, provider: Option<&str>, version: Option<&str>) {
    if let Some(report) = state.last_update_report.lock().await.as_mut() {
        for e in report.results.iter_mut().filter(|e| e.guid == old_guid) {
            if provider.is_none_or(|p| e.provider.eq_ignore_ascii_case(p)) {
                e.status = UpdateState::UpToDate;
                if let Some(v) = version {
                    e.installed_version = Some(v.to_string());
                } else {
                    e.installed_version = e.latest_version.clone();
                }
                e.files.clear();
                e.method = None;
                e.preselected_file = None;
            }
            e.guid = new_guid;
        }
    }
}

/// The newest version the last check found for `(guid, provider)` when it
/// reported an update -- used to record the installed version of an update
/// that arrived through the browser without one (e.g. from Nexus).
pub async fn known_latest_version(state: &AppState, guid: Uuid, provider: &str) -> Option<String> {
    let guard = state.last_update_report.lock().await;
    guard.as_ref()?.results.iter().find_map(|e| {
        (e.guid == guid
            && e.provider.eq_ignore_ascii_case(provider)
            && matches!(e.status, UpdateState::UpdateAvailable | UpdateState::Skipped))
        .then(|| e.latest_version.clone())
        .flatten()
    })
}

/// Whether the last check found an update for `(guid, provider)` -- lets
/// the extension show "Update with DDMM" on sites whose pages don't expose
/// a version (Nexus), based on DDMM's own check rather than a guess.
pub async fn known_update_available(state: &AppState, guid: Uuid, provider: &str) -> bool {
    let guard = state.last_update_report.lock().await;
    guard.as_ref().is_some_and(|r| {
        r.results
            .iter()
            .any(|e| e.guid == guid && e.provider.eq_ignore_ascii_case(provider) && e.status == UpdateState::UpdateAvailable)
    })
}

/// Best-effort "what version is this?" for a freshly installed mod, so its
/// first update check has something to compare against. Nexus archives
/// carry their version and upload time in their own file name (no API
/// call); a GitHub release-asset `download_url` names its release tag (the
/// same rule Add URL uses); otherwise the keyless sites are asked for their
/// current version. Never fails an install.
pub async fn enrich_install_source(
    source: &Source,
    archive_path: Option<&Path>,
    download_url: Option<&str>,
) -> (Source, Vec<InstalledFile>) {
    let mut source = source.clone();
    let mut files = Vec::new();
    let provider = source.provider.to_ascii_lowercase();
    let file_name = archive_path.and_then(|p| p.file_name()).and_then(|n| n.to_str()).map(str::to_string);

    // The release the user actually downloaded from, not whatever is the
    // latest release now (which may be a different, newer tag).
    if provider == "github" {
        if let Some(tag) = download_url.and_then(sources::github_tag_from_download_url) {
            source.version = Some(tag);
        }
    }

    if provider == "nexus" {
        if let Some(name) = &file_name {
            if let Some(parsed) = sources::parse_nexus_archive_name_for(name, source.id.as_deref()) {
                if source.id.as_deref().is_none_or(|id| id == parsed.mod_id) {
                    if source.version.is_none() {
                        source.version = Some(parsed.version.clone());
                    }
                    files.push(InstalledFile {
                        provider: "nexus".into(),
                        file_id: None,
                        file_name: Some(name.clone()),
                        label: None,
                        uploaded_at: parsed.uploaded_at,
                    });
                }
            }
        }
        return (source, files);
    }

    if let Some(name) = file_name {
        files.push(InstalledFile { provider: provider.clone(), file_name: Some(name), ..Default::default() });
    }
    if provider == "ayakamods" {
        // Always asked, even when the extension already read the version off
        // the page: the page's last-update time is what later checks compare
        // (two same-day updates share a version; see `ayakamods_status`).
        // A version from the page the user downloaded from is kept.
        let Some(id) = source.id.clone() else { return (source, files) };
        let Ok(client) = providers::build_client() else { return (source, files) };
        match ayakamods::fetch_ayakamods_metadata(&client, &id).await {
            Ok(Some(meta)) => {
                if source.version.is_none() {
                    source.version = meta.latest_version;
                }
                if let Some(modified) = meta.modified_at {
                    match files.iter_mut().find(|f| f.provider == provider) {
                        Some(f) => f.uploaded_at = Some(modified),
                        None => files.push(InstalledFile {
                            provider: provider.clone(),
                            uploaded_at: Some(modified),
                            ..Default::default()
                        }),
                    }
                }
            }
            Ok(None) => log::warn!("AyakaMods mod {id}: no version on its page to record."),
            // Not fatal: the install goes ahead, and update checks then
            // compare against when DDMM installed it.
            Err(e) => log::warn!("Couldn't record the AyakaMods page of mod {id}: {e}"),
        }
        return (source, files);
    }
    if source.version.is_some() {
        return (source, files);
    }
    let Some(id) = source.id.clone() else { return (source, files) };
    let Ok(client) = providers::build_client() else { return (source, files) };
    source.version = match provider.as_str() {
        "github" => github::latest_release(&client, &id).await.ok().flatten().and_then(|r| r.tag_name),
        "gamebanana" => gamebanana::fetch_mod(&client, &id).await.ok().and_then(|m| gamebanana::latest_version(&m)),
        "modworkshop" => modworkshop::fetch_mod(&client, &id).await.ok().and_then(|m| modworkshop::latest_version(&m)),
        _ => None,
    };
    (source, files)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ProgressEvent {
    guid: Uuid,
    downloaded: u64,
    total: Option<u64>,
}

/// One-click update for sites with public direct downloads: download the
/// chosen file (validated host, size cap, archive sniffing), then replace
/// the mod in place -- same GUID and profile config, same archive
/// validation as every other install -- and record the new version.
#[tauri::command]
pub async fn update_mod_direct(
    app: AppHandle,
    state: State<'_, AppState>,
    guid: Uuid,
    provider: String,
    file: UpdateFile,
    version: Option<String>,
) -> TAResult<InstalledMod> {
    let _data_op = state.data_op().into_ta_result()?;
    let provider = provider.to_ascii_lowercase();
    if !DIRECT_PROVIDERS.contains(&provider.as_str()) {
        return anyhow::anyhow!("{provider} updates go through the browser").into_ta_result();
    }
    if !providers::is_allowed_download_host(&provider, &file.url) {
        return anyhow::anyhow!("refusing to download an update for a {provider} mod from {}", crate::download::redact_url(&file.url))
            .into_ta_result();
    }

    let (mod_dir, source_id) = {
        let mut guard = state.mods.lock().await;
        let mods = ensure_mods_loaded(&mut guard, &state.base_path).await?;
        let m = mods
            .iter()
            .find(|m| m.guid() == guid)
            .ok_or_else(|| anyhow::anyhow!("mod {{{guid}}} not found"))
            .into_ta_result()?;
        let id = m
            .sources
            .iter()
            .filter(|s| s.provider.eq_ignore_ascii_case(&provider))
            .find_map(sources::resolved_source_id)
            .ok_or_else(|| anyhow::anyhow!("this mod has no {provider} source"))
            .into_ta_result()?;
        (m.directory.clone(), id)
    };

    log::info!("Updating mod {{{guid}}} from {provider} ({})...", crate::download::redact_url(&file.url));
    let staging_root = state.base_path.join(crate::download::STAGING_DIRECTORY);
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    let app_for_progress = app.clone();
    let downloaded = crate::download::download_archive_with_progress(&file.url, &staging_root, move |done, total| {
        let finished = total.is_some_and(|t| done >= t);
        if finished || done == 0 || last_emit.elapsed() >= Duration::from_millis(100) {
            last_emit = std::time::Instant::now();
            let _ = app_for_progress.emit(PROGRESS_EVENT, ProgressEvent { guid, downloaded: done, total });
        }
    })
    .await
    .into_ta_result()?;

    let result = install_direct_update(&state, guid, &downloaded.path, &provider, source_id, version.clone(), &file).await;
    let _ = tokio::fs::remove_dir_all(&downloaded.temp_dir).await;
    let (r#mod, warning) = result?;
    debug_assert_eq!(r#mod.directory, mod_dir);

    mark_mod_updated(&state, guid, r#mod.guid(), Some(&provider), version.as_deref()).await;
    log::info!("Mod {{{}}} updated from {provider} to {:?}.", r#mod.guid(), version);
    Ok(InstalledMod { r#mod, warning })
}

/// The install half of [`update_mod_direct`]: replace mod `guid` with
/// `archive` and record where it came from and exactly what was installed.
///
/// All with the mod list locked, like the browser updates: an update check
/// records AyakaMods page times (and skip times) into the sidecar with the
/// list locked. The old sidecar is read here, not before the download, so
/// whatever a check recorded meanwhile (another site's page time) is kept;
/// only this provider's entries change.
async fn install_direct_update(
    state: &AppState,
    guid: Uuid,
    archive: &Path,
    provider: &str,
    source_id: String,
    version: Option<String>,
    file: &UpdateFile,
) -> TAResult<(Mod, Option<String>)> {
    let mut guard = state.mods.lock().await;
    let Some(mods) = guard.as_mut() else { return anyhow::anyhow!("mods not read").into_ta_result() };
    let old_sidecar = match mods.iter().find(|m| m.guid() == guid) {
        Some(m) => sources::load_origin_sidecar(&m.directory).await,
        None => None,
    };
    let (mut r#mod, warning) = install_update_from_archive(state, mods, archive, guid).await?;

    let mut recorded_sources: Vec<Source> = old_sidecar.as_ref().map(|s| s.sources.clone()).unwrap_or_default();
    recorded_sources.retain(|s| !s.provider.eq_ignore_ascii_case(provider));
    recorded_sources.push(Source { provider: provider.to_string(), id: Some(source_id), url: None, version });
    let mut installed_files: Vec<InstalledFile> = old_sidecar.map(|s| s.installed_files).unwrap_or_default();
    installed_files.retain(|f| !f.provider.eq_ignore_ascii_case(provider));
    installed_files.push(InstalledFile {
        provider: provider.to_string(),
        file_id: Some(file.id.clone()),
        file_name: Some(file.name.clone()),
        label: file.label.clone(),
        uploaded_at: file.uploaded_at,
    });
    if let Err(e) = sources::write_origin_sidecar_with_files(&r#mod.directory, recorded_sources, installed_files).await {
        log::error!("Failed to write origin sidecar after update: {e}");
    }
    r#mod.resolve_sources().await;
    if let Some(existing) = mods.iter_mut().find(|m| m.guid() == r#mod.guid()) {
        *existing = r#mod.clone();
    }
    Ok((r#mod, warning))
}

/// Whether the browser extension has talked to this DDMM session recently
/// -- if so, a browser-based update can finish with its "Update with DDMM"
/// button instead of the Downloads-folder handoff.
#[tauri::command]
pub async fn browser_extension_active(state: State<'_, AppState>) -> TAResult<bool> {
    const RECENT: Duration = Duration::from_secs(12 * 60 * 60);
    Ok(state.bridge_last_seen.lock().await.is_some_and(|t| t.elapsed() < RECENT))
}

/// Opt-in automatic checks: once at startup and/or every N hours while
/// open -- only if the user turned them on in Settings (off by default).
/// Settings are re-read every minute, so toggling takes effect without a
/// restart (the startup check itself only happens at startup).
pub fn spawn_auto_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let the window come up first.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let base_path = app.state::<AppState>().base_path.clone();

        match do_load_settings(&base_path).await {
            Ok(s) if s.auto_check_updates() => run_and_emit(&app, CheckTrigger::Startup).await,
            Ok(_) => log::info!("Automatic update checks are off (Settings); not checking at startup."),
            Err(e) => log::warn!("Couldn't read settings for automatic update checks: {e:#}"),
        }

        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if app.state::<AppState>().data_ops_paused() {
                continue; // the data folder is being moved
            }
            let Ok(settings) = do_load_settings(&base_path).await else { continue };
            let Some(hours) = settings.auto_check_interval_hours().filter(|_| settings.auto_check_updates()) else {
                continue;
            };
            let due = {
                let state = app.state::<AppState>();
                let last = *state.last_update_check.lock().await;
                last.is_none_or(|t| t.elapsed() >= Duration::from_secs(u64::from(hours) * 3600))
            };
            if due {
                run_and_emit(&app, CheckTrigger::Scheduled).await;
            }
        }
    });
}

async fn run_and_emit(app: &AppHandle, trigger: CheckTrigger) {
    let state = app.state::<AppState>();
    match run_check(&state, trigger).await {
        Ok(report) => {
            if let Err(e) = app.emit(CHECKED_EVENT, &report) {
                log::error!("Failed to emit {CHECKED_EVENT}: {e}");
            }
        }
        Err(e) => {
            log::warn!("Automatic update check failed: {e:#}");
            // Don't retry every minute.
            *state.last_update_check.lock().await = Some(tokio::time::Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets;
    use crate::sources::ResolvedSource;

    #[test]
    fn status_compares_normalized_versions() {
        assert_eq!(status_for(Some("1.0"), Some("1.0")), UpdateState::UpToDate);
        assert_eq!(status_for(Some("v1.0"), Some("1.0.0")), UpdateState::UpToDate);
        assert_eq!(status_for(Some("1.0"), Some("2.0")), UpdateState::UpdateAvailable);
        assert_eq!(status_for(Some("2.0"), Some("1.9")), UpdateState::UpToDate);
        assert_eq!(status_for(None, Some("2.0")), UpdateState::Unknown);
        assert_eq!(status_for(Some("1.0"), None), UpdateState::Unknown);
        assert_eq!(status_for(Some("1.0"), Some("  ")), UpdateState::Unknown);
    }

    fn ayaka_meta(version: Option<&str>, modified_at: Option<i64>) -> ayakamods::AyakaModsMetadata {
        ayakamods::AyakaModsMetadata {
            name: None,
            latest_version: version.map(str::to_string),
            modified: None,
            modified_at,
            server_time: None,
        }
    }

    fn st(
        version: Option<&str>,
        recorded: Option<i64>,
        installed_at: Option<i64>,
        meta: &ayakamods::AyakaModsMetadata,
    ) -> UpdateState {
        ayakamods_status(version, recorded, installed_at, 0, meta).state
    }

    /// Issue #59: the Bolt Pistol page (fixture) says "2026-10-01" both
    /// for its 15:44 and its 22:08 release that day.
    #[test]
    fn ayakamods_same_day_updates_are_found_by_the_update_time() {
        let page = ayakamods::parse_ayakamods_page(include_str!("../../tests/fixtures/ayakamods_bolt_pistol_4101.html"))
            .unwrap();
        let page_time = page.modified_at.unwrap();
        // Installed the 15:44 release (recorded its page time then).
        let morning = page_time - 6 * 3600;
        assert_eq!(st(Some("2026-10-01"), Some(morning), None, &page), UpdateState::UpdateAvailable);
        // Installed the current one.
        assert_eq!(st(Some("2026-10-01"), Some(page_time), None, &page), UpdateState::UpToDate);
        // A different version string is an update on its own.
        assert_eq!(st(Some("2026-09-29"), Some(page_time), None, &page), UpdateState::UpdateAvailable);
        assert_eq!(st(Some("2026-09-29"), None, None, &page), UpdateState::UpdateAvailable);
    }

    /// Installed before this fix (or the page couldn't be read at
    /// install): the install time stands in for the page time.
    #[test]
    fn ayakamods_falls_back_to_the_install_time() {
        let t = 1_790_889_008;
        let meta = ayakamods::AyakaModsMetadata { modified_at: Some(t), ..ayaka_meta(Some("2026-10-01"), None) };
        // Installed in the morning, updated in the evening.
        assert_eq!(st(Some("2026-10-01"), None, Some(t - 6 * 3600), &meta), UpdateState::UpdateAvailable);
        // Installed after the update.
        assert_eq!(st(Some("2026-10-01"), None, Some(t + 60), &meta), UpdateState::UpToDate);
        // Installed a minute before the page's time settled: not an update.
        assert_eq!(st(Some("2026-10-01"), None, Some(t - 90), &meta), UpdateState::UpToDate);
        // No version recorded at all (the page was unreachable at install):
        // the install time alone still decides.
        assert_eq!(st(None, None, Some(t - 6 * 3600), &meta), UpdateState::UpdateAvailable);
        assert_eq!(st(None, None, Some(t + 60), &meta), UpdateState::UpToDate);
    }

    #[test]
    fn ayakamods_without_times_compares_versions() {
        let meta = ayaka_meta(Some("v1.7"), None);
        assert_eq!(st(Some("1.5"), None, None, &meta), UpdateState::UpdateAvailable);
        assert_eq!(st(Some("1.7"), None, None, &meta), UpdateState::UpToDate);
        assert_eq!(st(None, None, None, &meta), UpdateState::Unknown);
        assert_eq!(st(Some("1.7"), Some(5), None, &ayaka_meta(None, None)), UpdateState::Unknown);
        // A newer version installed by hand isn't an update.
        assert_eq!(st(Some("2.0"), None, None, &meta), UpdateState::UpToDate);
    }

    #[tokio::test]
    async fn ayakamods_targets_carry_the_recorded_page_time_and_install_time() {
        let dir = tempfile::tempdir().unwrap();
        let source = Source { provider: "ayakamods".into(), id: Some("4101".into()), url: None, version: Some("2026-10-01".into()) };
        sources::write_origin_sidecar_with_files(
            dir.path(),
            vec![source.clone()],
            vec![InstalledFile { provider: "ayakamods".into(), uploaded_at: Some(1_790_865_843), ..Default::default() }],
        )
        .await
        .unwrap();
        let mut resolved = sources::resolve(&source);
        resolved.origin = SourceOrigin::Install;
        let targets = collect_targets(&[mod_with_sources(dir.path(), vec![resolved])]).await;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "4101");
        assert_eq!(targets[0].installed_file.as_ref().and_then(|f| f.uploaded_at), Some(1_790_865_843));
        assert!(targets[0].installed_at.is_some_and(|t| t > 1_700_000_000));

        // A manifest-declared source with no install record has no install time.
        let dir2 = tempfile::tempdir().unwrap();
        let declared = sources::resolve(&source);
        sources::set_skipped_version(dir2.path(), "ayakamods", Some("x"), None).await.unwrap();
        let targets = collect_targets(&[mod_with_sources(dir2.path(), vec![declared])]).await;
        assert_eq!(targets[0].installed_at, None);
    }


    /// The PC's clock was an hour behind when the mod was installed: the
    /// site's clock (its `Date` header) corrects the install time, so a
    /// fresh install isn't flagged; the page time is then recorded as the
    /// baseline. A real pending update is flagged and nothing is recorded.
    #[test]
    fn ayakamods_install_time_is_corrected_for_a_wrong_clock() {
        let page = 1_790_889_008;
        let meta = ayakamods::AyakaModsMetadata { modified_at: Some(page), ..ayaka_meta(Some("2026-10-01"), None) };
        let installed_at = page + 120 - 3600; // installed 2 min after the update, clock 1 h behind
        let skewed = ayakamods_status(Some("2026-10-01"), None, Some(installed_at), 3600, &meta);
        assert_eq!(skewed, AyakaDecision { state: UpdateState::UpToDate, record_baseline: Some(page) });
        // Without the correction it would have been a false update.
        assert_eq!(st(Some("2026-10-01"), None, Some(installed_at), &meta), UpdateState::UpdateAvailable);
        // Network-delay-sized differences are ignored.
        assert_eq!(
            ayakamods_status(Some("2026-10-01"), None, Some(page - 700), 30, &meta).state,
            UpdateState::UpdateAvailable
        );
        // A pending update stays an update, and isn't recorded as a baseline.
        let pending = ayakamods_status(Some("2026-10-01"), None, Some(page - 6 * 3600), 0, &meta);
        assert_eq!(pending, AyakaDecision { state: UpdateState::UpdateAvailable, record_baseline: None });
        // Already recorded: nothing more to record.
        assert_eq!(ayakamods_status(Some("2026-10-01"), Some(page), None, 0, &meta).record_baseline, None);
    }

    /// Writes an AyakaMods mod as DDMM recorded it before page times were
    /// recorded: version and install time only.
    async fn add_old_ayakamods_mod(base: &Path, id: &str, installed_at: u64) -> (Uuid, std::path::PathBuf) {
        let mod_dir = base.join("mods").join(format!("ayaka-{id}"));
        tokio::fs::create_dir_all(&mod_dir).await.unwrap();
        let guid = Uuid::new_v4();
        tokio::fs::write(
            mod_dir.join("manifest.json"),
            format!(r#"{{"Guid":"{guid}","Name":"Ayaka {id}","Description":"","IconPath":null,"Options":null}}"#),
        )
        .await
        .unwrap();
        let sidecar = sources::OriginSidecar {
            sources: vec![Source { provider: "ayakamods".into(), id: Some(id.into()), url: None, version: Some("2026-10-01".into()) }],
            installed_at,
            installed_files: vec![InstalledFile { provider: "ayakamods".into(), file_name: Some("m.zip".into()), ..Default::default() }],
            skipped_versions: Vec::new(),
            imported_archive: None,
        };
        sources::save_origin_sidecar(&mod_dir, &sidecar).await.unwrap();
        (guid, mod_dir)
    }

    #[tokio::test]
    async fn the_first_check_records_the_page_time_as_the_baseline() {
        use ayakamods::test_support::{page, reply, server, BASE};
        let page_time = 1_790_889_008; // 2026-10-01T21:10:08Z
        let dir = tempfile::tempdir().unwrap();
        // Installed just after the update, but this PC's clock is 1 h behind.
        let (current, current_dir) = add_old_ayakamods_mod(dir.path(), "11", (page_time + 120 - 3600) as u64).await;
        // Installed hours before the update: a real pending update.
        let (pending, pending_dir) = add_old_ayakamods_mod(dir.path(), "12", (page_time - 6 * 3600) as u64).await;
        let date = {
            // The site's clock: the real "now", which is this PC's now
            // plus the hour it is behind.
            let real_now = now_unix() + 3600;
            http_date(real_now)
        };
        let (base, _seen) =
            server(vec![reply("200 OK", &format!("Date: {date}\r\n"), &page("2026-10-01", "2026-10-01T22:10:08+01:00"))])
                .await;
        let state = AppState::new(dir.path().to_path_buf());
        let factory = |key| NexusClient::new(key);

        let report = BASE.scope(base.clone(), run_check_with(&state, CheckTrigger::Manual, &factory)).await.unwrap();
        let status = |r: &UpdateCheckReport, g: Uuid| r.results.iter().find(|e| e.guid == g).unwrap().status.clone();
        assert_eq!(status(&report, current), UpdateState::UpToDate);
        assert_eq!(status(&report, pending), UpdateState::UpdateAvailable);
        let recorded = |d: &Path| {
            let d = d.to_path_buf();
            async move { sources::load_origin_sidecar(&d).await.unwrap().installed_files[0].uploaded_at }
        };
        assert_eq!(recorded(&current_dir).await, Some(page_time), "baseline recorded");
        assert_eq!(recorded(&pending_dir).await, None, "a pending update is never recorded as current");

        // From now on it's page time vs page time: even with the clock
        // wildly off (no usable Date header), no false update.
        let (base2, _seen2) = server(vec![reply("200 OK", "", &page("2026-10-01", "2026-10-01T22:10:08+01:00"))]).await;
        let report = BASE.scope(base2, run_check_with(&state, CheckTrigger::Manual, &factory)).await.unwrap();
        assert_eq!(status(&report, current), UpdateState::UpToDate);
        assert_eq!(status(&report, pending), UpdateState::UpdateAvailable, "still flagged until installed");
    }

    fn http_date(ts: i64) -> String {
        // `format_timestamp` gives "YYYY-MM-DD HH:MM UTC"; build the HTTP form.
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let f = providers::format_timestamp(ts);
        let (y, m, d) = (&f[0..4], f[5..7].parse::<usize>().unwrap(), &f[8..10]);
        let secs = ts.rem_euclid(60);
        format!("Thu, {d} {} {y} {}:{secs:02} GMT", MONTHS[m - 1], &f[11..16])
    }

    /// Issue #59 review: the extension always sends the page's version, and
    /// installs from it used to skip the page lookup, so the page time was
    /// never recorded. The extension's version is kept; the time is added.
    #[tokio::test]
    async fn extension_installs_still_record_the_ayakamods_page_time() {
        use ayakamods::test_support::{page, reply, server, BASE};
        let (base, seen) = server(vec![reply("200 OK", "", &page("2026-10-01", "2026-10-01T22:10:08+01:00"))]).await;
        let source = Source {
            provider: "ayakamods".into(),
            id: Some("13".into()),
            url: None,
            version: Some("2026-10-01-from-page".into()),
        };
        let (s, files) = BASE
            .scope(base, enrich_install_source(&source, Some(Path::new("/dl/Bolt-v1.7.zip")), None))
            .await;
        assert_eq!(s.version.as_deref(), Some("2026-10-01-from-page"));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name.as_deref(), Some("Bolt-v1.7.zip"));
        assert_eq!(files[0].uploaded_at, Some(1_790_889_008));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// Recording an AyakaMods baseline re-reads the sidecar at write time
    /// (with the mod list locked), so whatever was written to it since the
    /// check collected its targets -- another site's file, a skip -- is
    /// kept, never replaced by the copy read at the start of the check.
    #[tokio::test]
    async fn recording_a_baseline_keeps_what_was_written_since_the_check_started() {
        let dir = tempfile::tempdir().unwrap();
        let (_guid, mod_dir) = add_old_ayakamods_mod(dir.path(), "31", 1_789_990_000).await;
        let mut resolved = sources::resolve(&Source {
            provider: "ayakamods".into(),
            id: Some("31".into()),
            url: None,
            version: Some("2026-10-01".into()),
        });
        resolved.origin = SourceOrigin::Install;
        let targets = collect_targets(&[mod_with_sources(&mod_dir, vec![resolved])]).await;

        // Meanwhile: a one-click GitHub file and a skip are recorded.
        let mut sidecar = sources::load_origin_sidecar(&mod_dir).await.unwrap();
        sidecar.installed_files.push(InstalledFile { provider: "github".into(), file_name: Some("new.zip".into()), ..Default::default() });
        sources::save_origin_sidecar(&mod_dir, &sidecar).await.unwrap();
        sources::set_skipped_version(&mod_dir, "github", Some("v9"), None).await.unwrap();

        record_ayakamods_baseline(&targets[0], 1_790_000_000).await.unwrap();
        let after = sources::load_origin_sidecar(&mod_dir).await.unwrap();
        assert_eq!(after.installed_files.len(), 2);
        assert_eq!(after.installed_files[0].uploaded_at, Some(1_790_000_000));
        assert_eq!(after.installed_files[1].file_name.as_deref(), Some("new.zip"));
        assert_eq!(after.skipped_versions.len(), 1);
    }

    async fn loaded(state: &AppState) {
        let mut guard = state.mods.lock().await;
        ensure_mods_loaded(&mut guard, &state.base_path).await.unwrap();
    }

    /// "Skip" writes the sidecar with the mod list locked, so it can't
    /// interleave with an update check's (or update's) sidecar writes.
    #[tokio::test]
    async fn skipping_waits_for_the_mod_list() {
        let dir = tempfile::tempdir().unwrap();
        let (guid, mod_dir) = add_old_ayakamods_mod(dir.path(), "41", 1_789_990_000).await;
        let state = std::sync::Arc::new(AppState::new(dir.path().to_path_buf()));
        loaded(&state).await;
        let held = state.mods.lock().await;
        let task = {
            let state = state.clone();
            tokio::spawn(async move { set_skip(&state, guid, "ayakamods".into(), Some("1.0".into()), Some(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!task.is_finished(), "must wait for the mod list");
        assert!(sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions.is_empty());
        drop(held);
        task.await.unwrap().unwrap();
        assert_eq!(sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions, vec![skip("1.0", Some(5))]);
    }

    /// An AyakaMods update found by its page time alone (the page gives no
    /// version) can be skipped by that time, and a later one still shows.
    #[tokio::test]
    async fn an_update_without_a_version_can_be_skipped_by_its_page_time() {
        let dir = tempfile::tempdir().unwrap();
        let (guid, mod_dir) = add_old_ayakamods_mod(dir.path(), "42", 1_789_990_000).await;
        let state = AppState::new(dir.path().to_path_buf());
        loaded(&state).await;
        let page_time = 1_790_889_008;
        let e = UpdateStatusEntry {
            guid,
            provider: "ayakamods".into(),
            display_name: "AyakaMods".into(),
            source_id: Some("42".into()),
            installed_version: Some("2026-10-01".into()),
            latest_version: None,
            latest_file_name: None,
            latest_modified_at: Some(page_time),
            status: UpdateState::UpdateAvailable,
            page_url: None,
            method: Some(UpdateMethod::Browser),
            files: vec![],
            preselected_file: None,
        };
        *state.last_update_report.lock().await = Some(UpdateCheckReport {
            trigger: CheckTrigger::Manual,
            checked_at: 0,
            results: vec![e],
            nexus_rate_limit: None,
            nexus_checked_recently: false,
        });

        // What the frontend sends: no version, the page time.
        set_skip(&state, guid, "ayakamods".into(), None, Some(page_time)).await.unwrap();
        let stored = sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions;
        assert_eq!(stored, vec![skip("", Some(page_time))]);
        let report = state.last_update_report.lock().await.clone().unwrap();
        assert_eq!(report.results[0].status, UpdateState::Skipped);

        assert!(skip_covers(&stored[0], None, Some(page_time)));
        assert!(!skip_covers(&stored[0], None, Some(page_time + 1)), "a later update shows");
        assert!(!skip_covers(&stored[0], None, None));

        // Stop skipping (neither version nor time) clears it.
        set_skip(&state, guid, "ayakamods".into(), None, None).await.unwrap();
        assert!(sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions.is_empty());
        let report = state.last_update_report.lock().await.clone().unwrap();
        assert_eq!(report.results[0].status, UpdateState::UpdateAvailable);
    }

    /// A one-click update re-reads the sidecar with the mod list locked, so
    /// an AyakaMods page time a check recorded during the download is kept;
    /// only the updated site's entries change.
    #[tokio::test]
    async fn a_one_click_update_keeps_what_a_check_recorded_during_the_download() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let (guid, mod_dir) = add_old_ayakamods_mod(dir.path(), "43", 1_789_990_000).await;
        let mut sidecar = sources::load_origin_sidecar(&mod_dir).await.unwrap();
        sidecar.sources.push(Source { provider: "github".into(), id: Some("o/r".into()), url: None, version: Some("v1".into()) });
        sidecar.installed_files.push(InstalledFile { provider: "github".into(), file_name: Some("old.zip".into()), ..Default::default() });
        sources::save_origin_sidecar(&mod_dir, &sidecar).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        loaded(&state).await;

        // "During the download": a check records the AyakaMods baseline.
        let mut sidecar = sources::load_origin_sidecar(&mod_dir).await.unwrap();
        sidecar.installed_files[0].uploaded_at = Some(1_790_000_000);
        sources::save_origin_sidecar(&mod_dir, &sidecar).await.unwrap();

        let archive = dir.path().join("new.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            w.start_file("0123456789abcdef.patch_0", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"new").unwrap();
            w.finish().unwrap();
        }
        let file = UpdateFile {
            id: "a1".into(),
            name: "new.zip".into(),
            label: None,
            size: None,
            uploaded_at: Some(1_791_000_000),
            url: "https://github.com/o/r/releases/download/v2/new.zip".into(),
        };
        let (m, _) =
            install_direct_update(&state, guid, &archive, "github", "o/r".into(), Some("v2".into()), &file).await.unwrap();

        let after = sources::load_origin_sidecar(&m.directory).await.unwrap();
        let ayaka = after.installed_files.iter().find(|f| f.provider == "ayakamods").unwrap();
        assert_eq!(ayaka.uploaded_at, Some(1_790_000_000), "baseline kept");
        let gh = after.installed_files.iter().find(|f| f.provider == "github").unwrap();
        assert_eq!(gh.file_name.as_deref(), Some("new.zip"));
        assert_eq!(after.installed_files.len(), 2);
        let gh_src = after.sources.iter().find(|s| s.provider == "github").unwrap();
        assert_eq!(gh_src.version.as_deref(), Some("v2"));
        assert!(after.sources.iter().any(|s| s.provider == "ayakamods"));
    }

    fn skip(version: &str, modified_at: Option<i64>) -> sources::SkippedVersion {
        sources::SkippedVersion { provider: "ayakamods".into(), version: version.into(), modified_at }
    }

    #[test]
    fn a_skip_with_a_page_time_covers_only_that_update() {
        let skipped_at = 1_790_889_008;
        let s = skip("2026-10-01", Some(skipped_at));
        assert!(skip_covers(&s, Some("2026-10-01"), Some(skipped_at)));
        assert!(skip_covers(&s, Some("v2026-10-01"), Some(skipped_at - 60)));
        // Same version string, page updated since: a new update.
        assert!(!skip_covers(&s, Some("2026-10-01"), Some(skipped_at + 1)));
        // A different version is never covered.
        assert!(!skip_covers(&s, Some("2026-10-02"), Some(skipped_at)));
        assert!(!skip_covers(&s, None, Some(skipped_at)));
        // No page time this check: the version decides.
        assert!(skip_covers(&s, Some("2026-10-01"), None));
        // A skip saved before times were recorded (or another site): the
        // version decides.
        assert!(skip_covers(&skip("1.0", None), Some("1.0"), Some(skipped_at)));
        assert!(!skip_covers(&skip("1.0", None), Some("1.1"), None));
    }

    /// QA repro: page time recorded at install 1_790_000_000, "2026-10-01"
    /// skipped (its page time recorded with it), then the page is updated
    /// on 2026-12-24 still saying "2026-10-01": that's a new update, not
    /// the skipped one.
    #[tokio::test]
    async fn skipping_an_ayakamods_update_does_not_hide_later_ones_with_the_same_version() {
        use ayakamods::test_support::{page, reply, server, BASE};
        let dir = tempfile::tempdir().unwrap();
        let (guid, mod_dir) = add_old_ayakamods_mod(dir.path(), "21", 1_789_990_000).await;
        let mut sidecar = sources::load_origin_sidecar(&mod_dir).await.unwrap();
        sidecar.installed_files[0].uploaded_at = Some(1_790_000_000);
        sources::save_origin_sidecar(&mod_dir, &sidecar).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let factory = |key| NexusClient::new(key);
        let status = |r: &UpdateCheckReport| r.results.iter().find(|e| e.guid == guid).unwrap().status.clone();
        let check = |date: &'static str| {
            let state = &state;
            async move {
                let (base, _seen) = server(vec![reply("200 OK", "", &page("2026-10-01", date))]).await;
                BASE.scope(base, run_check_with(state, CheckTrigger::Manual, &factory)).await.unwrap()
            }
        };

        let report = check("2026-10-01T22:10:08+01:00").await;
        assert_eq!(status(&report), UpdateState::UpdateAvailable);
        let e = report.results.iter().find(|e| e.guid == guid).unwrap();
        assert_eq!(e.latest_modified_at, Some(1_790_889_008));

        // "Skip this version" without a time (an older frontend): the time
        // comes from the last check.
        set_skip(&state, guid, "ayakamods".into(), Some("2026-10-01".into()), None).await.unwrap();
        let stored = sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions;
        assert_eq!(stored, vec![skip("2026-10-01", Some(1_790_889_008))]);
        let last = state.last_update_report.lock().await.clone().unwrap();
        assert_eq!(status(&last), UpdateState::Skipped);

        // The same update: still skipped.
        assert_eq!(status(&check("2026-10-01T22:10:08+01:00").await), UpdateState::Skipped);
        // A later update with the same version string: shown.
        assert_eq!(status(&check("2026-12-24T10:00:00+00:00").await), UpdateState::UpdateAvailable);

        // Stop skipping clears it.
        set_skip(&state, guid, "ayakamods".into(), None, None).await.unwrap();
        assert!(sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions.is_empty());

        // The frontend's own time wins over the report's.
        set_skip(&state, guid, "ayakamods".into(), Some("2026-10-01".into()), Some(1_798_106_400)).await.unwrap();
        let stored = sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions;
        assert_eq!(stored, vec![skip("2026-10-01", Some(1_798_106_400))]);
        assert_eq!(status(&check("2026-12-24T10:00:00+00:00").await), UpdateState::Skipped);
    }

    /// A skip saved before page times were recorded keeps hiding the update
    /// it was made for, takes that update's page time, and so no longer
    /// hides the page's next update.
    #[tokio::test]
    async fn an_old_skip_without_a_page_time_heals_on_the_next_check() {
        use ayakamods::test_support::{page, reply, server, BASE};
        let dir = tempfile::tempdir().unwrap();
        let (guid, mod_dir) = add_old_ayakamods_mod(dir.path(), "22", 1_789_990_000).await;
        let mut sidecar = sources::load_origin_sidecar(&mod_dir).await.unwrap();
        sidecar.installed_files[0].uploaded_at = Some(1_790_000_000);
        sources::save_origin_sidecar(&mod_dir, &sidecar).await.unwrap();
        sources::set_skipped_version(&mod_dir, "ayakamods", Some("2026-10-01"), None).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let factory = |key| NexusClient::new(key);
        let status = |r: &UpdateCheckReport| r.results.iter().find(|e| e.guid == guid).unwrap().status.clone();

        let (base, _seen) = server(vec![reply("200 OK", "", &page("2026-10-01", "2026-10-01T22:10:08+01:00"))]).await;
        let report = BASE.scope(base, run_check_with(&state, CheckTrigger::Manual, &factory)).await.unwrap();
        assert_eq!(status(&report), UpdateState::Skipped);
        let stored = sources::load_origin_sidecar(&mod_dir).await.unwrap().skipped_versions;
        assert_eq!(stored, vec![skip("2026-10-01", Some(1_790_889_008))]);

        let (base, _seen) = server(vec![reply("200 OK", "", &page("2026-10-01", "2026-12-24T10:00:00+00:00"))]).await;
        let report = BASE.scope(base, run_check_with(&state, CheckTrigger::Manual, &factory)).await.unwrap();
        assert_eq!(status(&report), UpdateState::UpdateAvailable);
    }

    #[test]
    fn skipped_versions_match_ignoring_a_leading_v() {
        assert!(same_version("v1.2", "1.2"));
        assert!(same_version("1.2", "V1.2"));
        assert!(same_version("1-2", "1.2"));
        assert!(!same_version("1.2", "1.3"));
    }

    fn file(id: &str, name: &str, label: Option<&str>) -> UpdateFile {
        UpdateFile {
            id: id.into(),
            name: name.into(),
            label: label.map(str::to_string),
            size: None,
            uploaded_at: None,
            url: format!("https://gamebanana.com/dl/{id}"),
        }
    }

    #[test]
    fn preselects_only_when_unambiguous() {
        let one = vec![file("1", "mod.zip", None)];
        assert_eq!(preselect(&one, None).as_deref(), Some("1"));

        let many = vec![
            file("1", "rabu_ss_sa-8_no_helm_8430d.zip", Some("SA-8 NO HELM")),
            file("2", "rabu_ss_dp-00_1acbd.zip", Some("DP-00")),
            file("3", "rabu_ss_sa-8_ff3bb.zip", Some("SA-8")),
        ];
        assert_eq!(preselect(&many, None), None, "several files and nothing known: ask");

        let by_label = InstalledFile { provider: "gamebanana".into(), label: Some("dp-00".into()), ..Default::default() };
        assert_eq!(preselect(&many, Some(&by_label)).as_deref(), Some("2"));

        let by_name = InstalledFile {
            provider: "gamebanana".into(),
            file_name: Some("rabu_ss_sa-8_no_helm_00aa1.zip".into()),
            ..Default::default()
        };
        assert_eq!(preselect(&many, Some(&by_name)).as_deref(), Some("1"));
    }

    fn mod_with_sources(dir: &Path, sources: Vec<ResolvedSource>) -> Mod {
        Mod {
            manifest: crate::models::manifest::Manifest::Legacy(crate::models::manifest::legacy::Manifest {
                guid: Uuid::new_v4(),
                name: "M".into(),
                description: String::new(),
                icon_path: None,
                options: None,
            }),
            directory: dir.to_path_buf(),
            sources,
        }
    }

    #[tokio::test]
    async fn targets_dedupe_and_prefer_the_recorded_install_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest_src = sources::resolve(&Source {
            provider: "gamebanana".into(),
            id: Some("5".into()),
            url: None,
            version: Some("1.0".into()),
        });
        manifest_src.origin = SourceOrigin::Manifest;
        let mut install_src = sources::resolve(&Source {
            provider: "gamebanana".into(),
            id: Some("5".into()),
            url: None,
            version: Some("1.1".into()),
        });
        install_src.origin = SourceOrigin::Install;
        let unsupported = sources::resolve(&Source {
            provider: "url".into(),
            id: None,
            url: Some("https://example.com/x.zip".into()),
            version: None,
        });
        let m = mod_with_sources(dir.path(), vec![manifest_src, install_src, unsupported]);

        sources::set_skipped_version(dir.path(), "gamebanana", Some("1.2"), None).await.unwrap();
        let targets = collect_targets(&[m]).await;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "5");
        assert_eq!(targets[0].installed_version.as_deref(), Some("1.1"));
        assert_eq!(targets[0].skipped.as_ref().map(|s| s.version.as_str()), Some("1.2"));
    }

    #[tokio::test]
    async fn nexus_without_a_key_needs_one_and_makes_no_requests() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target {
            guid: Uuid::new_v4(),
            provider: "nexus".into(),
            id: "1234".into(),
            display_name: "Nexus Mods".into(),
            page_url: None,
            installed_version: Some("1.0".into()),
            installed_file: None,
            skipped: None,
            installed_at: None,
            directory: dir.path().to_path_buf(),
        };
        // No fallback file in this data dir. (On a dev machine with a real
        // keychain entry this would find it -- so only assert when none.)
        if secrets::load(dir.path()).await.is_none() {
            let outcome = check_nexus(dir.path(), &[&t], &NexusClient::new).await;
            assert_eq!(outcome.entries[0].status, UpdateState::NeedsApiKey);
            assert!(outcome.rate.is_none());
        }
    }

    #[tokio::test]
    async fn nexus_archive_names_record_version_and_upload_time_offline() {
        let source = Source { provider: "nexus".into(), id: Some("1234".into()), url: None, version: None };
        let (s, files) = enrich_install_source(&source, Some(Path::new("/dl/Better Stims-1234-1-1-1718100000.zip")), None).await;
        assert_eq!(s.version.as_deref(), Some("1.1"));
        assert_eq!(files[0].uploaded_at, Some(1718100000));

        // A file from a *different* Nexus mod never stamps this one.
        let (s, files) = enrich_install_source(&source, Some(Path::new("/dl/Other-99-2-0-1718100000.zip")), None).await;
        assert_eq!(s.version, None);
        assert!(files.is_empty());
    }

    /// An extension install of an older GitHub release records *that*
    /// release's tag (from the download URL), never the latest one -- and
    /// asks GitHub nothing to find it.
    #[tokio::test]
    async fn github_bridge_installs_record_the_downloaded_release_tag() {
        let source = Source { provider: "github".into(), id: Some("owner/repo".into()), url: None, version: None };
        let (s, files) = enrich_install_source(
            &source,
            Some(Path::new("/dl/cool-mod.zip")),
            Some("https://github.com/owner/repo/releases/download/v1.2.0/cool-mod.zip"),
        )
        .await;
        assert_eq!(s.version.as_deref(), Some("v1.2.0"));
        assert_eq!(files[0].file_name.as_deref(), Some("cool-mod.zip"));
    }

    /// A mock Nexus API recording every request path. `updated.json`
    /// answers with `updated_body`; any `files.json` with the recorded
    /// files fixture.
    async fn mock_nexus(updated_body: String) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let files = include_str!("../../tests/fixtures/updates/nexus_files.json");
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                let body = if path.contains("/updated.json") { updated_body.clone() } else { files.to_string() };
                log.lock().unwrap().push(path);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nx-rl-hourly-remaining: 499\r\nx-rl-daily-remaining: 19999\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}/v1/"), seen)
    }

    async fn add_nexus_mod(base: &Path, mod_id: &str) -> Uuid {
        let mod_dir = base.join("mods").join(format!("nexus-{mod_id}"));
        tokio::fs::create_dir_all(&mod_dir).await.unwrap();
        let guid = Uuid::new_v4();
        tokio::fs::write(
            mod_dir.join("manifest.json"),
            format!(r#"{{"Guid":"{guid}","Name":"Nexus {mod_id}","Description":"","IconPath":null,"Options":null}}"#),
        )
        .await
        .unwrap();
        sources::write_origin_sidecar_with_files(
            &mod_dir,
            vec![Source { provider: "nexus".into(), id: Some(mod_id.into()), url: None, version: Some("1.0".into()) }],
            vec![InstalledFile { provider: "nexus".into(), uploaded_at: Some(1718000000), ..Default::default() }],
        )
        .await
        .unwrap();
        guid
    }

    /// Pretend every Nexus mod in `state` was last checked `age` seconds
    /// ago (as a real earlier check would have recorded).
    async fn backdate_nexus_checks(state: &AppState, age: i64) {
        let mut cache = nexus::load_cache(&state.base_path).await;
        for c in cache.nexus.values_mut() {
            c.checked_at = now_unix() - age;
        }
        nexus::save_cache(&state.base_path, &cache).await;
    }

    fn files_requests(seen: &std::sync::Mutex<Vec<String>>) -> Vec<String> {
        seen.lock().unwrap().iter().filter(|p| p.ends_with("/files.json")).cloned().collect()
    }

    /// Set up a data dir with one installed Nexus mod and a stored key.
    async fn nexus_only_state() -> (tempfile::TempDir, AppState, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let mod_dir = dir.path().join("mods").join("nexus-mod");
        tokio::fs::create_dir_all(&mod_dir).await.unwrap();
        let guid = Uuid::new_v4();
        tokio::fs::write(
            mod_dir.join("manifest.json"),
            format!(r#"{{"Guid":"{guid}","Name":"Better Stims","Description":"","IconPath":null,"Options":null}}"#),
        )
        .await
        .unwrap();
        sources::write_origin_sidecar_with_files(
            &mod_dir,
            vec![Source { provider: "nexus".into(), id: Some("1234".into()), url: None, version: Some("1.0".into()) }],
            vec![InstalledFile { provider: "nexus".into(), uploaded_at: Some(1718000000), ..Default::default() }],
        )
        .await
        .unwrap();
        // The fallback file is read before the OS keychain, so this key is
        // the one used whatever the machine's keychain holds.
        tokio::fs::write(dir.path().join(secrets::FALLBACK_FILE_NAME), "TestKeyForAupCheck").await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        (dir, state, guid)
    }

    /// A mock Nexus API that rejects every request (revoked credentials).
    async fn mock_nexus_rejecting() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let body = r#"{"message":"Token revoked"}"#;
                let resp = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}/v1/")
    }

    #[tokio::test]
    async fn signed_in_checks_use_the_sign_in_over_the_key_and_sign_out_when_revoked() {
        use crate::secrets::Secret;
        use std::sync::{Arc, Mutex};
        let (_dir, state, guid) = nexus_only_state().await; // also has a personal key
        let tokens = nexus_oauth::Tokens {
            access_token: Secret::new("ACCESS-TOKEN-UPD"),
            refresh_token: Some(Secret::new("REFRESH-TOKEN-UPD")),
            expires_at: now_unix() + 3600,
            scope: nexus_oauth::SCOPES.into(),
            username: Some("Diver".into()),
            is_premium: false,
        };
        nexus_oauth::store_tokens(&state.base_path, &tokens).await.unwrap();

        let used: Arc<Mutex<Vec<NexusAuth>>> = Arc::default();
        let (base, seen) = mock_nexus("[]".into()).await;
        let log = used.clone();
        let factory = move |auth: NexusAuth| {
            log.lock().unwrap().push(auth.clone());
            NexusClient::with_base(auth, &base)
        };
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert_eq!(used.lock().unwrap().as_slice(), &[NexusAuth::OAuth(Secret::new("ACCESS-TOKEN-UPD"))]);
        assert!(!seen.lock().unwrap().is_empty());
        assert!(report.results.iter().any(|e| e.guid == guid && e.status == UpdateState::UpdateAvailable));

        // Revoked on Nexus's side: the API answers 401 -> signed out, told why.
        backdate_nexus_checks(&state, 40 * 86_400).await;
        let rejecting = mock_nexus_rejecting().await;
        let factory = move |auth: NexusAuth| NexusClient::with_base(auth, &rejecting);
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        let nexus = report.results.iter().find(|e| e.guid == guid).unwrap();
        match &nexus.status {
            UpdateState::Error { message } => {
                assert_eq!(message, nexus_oauth::SIGNED_OUT_MESSAGE);
                assert!(!message.contains("ACCESS-TOKEN-UPD"));
            }
            other => panic!("{other:?}"),
        }
        assert!(nexus_oauth::load_tokens(&state.base_path).await.is_none(), "sign-in removed");

        // The personal key takes over from then on.
        let used: Arc<Mutex<Vec<NexusAuth>>> = Arc::default();
        let log = used.clone();
        let (base, _seen) = mock_nexus("[]".into()).await;
        let factory = move |auth: NexusAuth| {
            log.lock().unwrap().push(auth.clone());
            NexusClient::with_base(auth, &base)
        };
        backdate_nexus_checks(&state, 40 * 86_400).await;
        run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(matches!(used.lock().unwrap().as_slice(), [NexusAuth::ApiKey(_)]));
    }

    /// A mod deleted while a check waits on the network doesn't get its
    /// update status back when the check stores its report.
    #[tokio::test]
    async fn a_mod_deleted_during_a_check_is_left_out_of_its_report() {
        let (base, _seen) = mock_nexus("[]".into()).await;
        let (_dir, state, guid) = nexus_only_state().await;
        let state = std::sync::Arc::new(state);
        let during = state.clone();
        let factory = move |auth: NexusAuth| {
            // The check is past reading the mod list: delete the mod now.
            if let Ok(mut mods) = during.mods.try_lock() {
                if let Some(mods) = mods.as_mut() {
                    mods.retain(|m| m.guid() != guid);
                }
            }
            NexusClient::with_base(auth, &base)
        };
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(report.results.iter().all(|e| e.guid != guid), "{:?}", report.results);
        let stored = state.last_update_report.lock().await.clone().unwrap();
        assert!(stored.results.iter().all(|e| e.guid != guid));
    }

    #[tokio::test]
    async fn automatic_checks_make_zero_nexus_api_requests() {
        let (base, seen) = mock_nexus("[]".into()).await;
        let count = || seen.lock().unwrap().len();
        let factory = move |key| NexusClient::with_base(key, &base);

        let (_dir, state, guid) = nexus_only_state().await;
        for trigger in [CheckTrigger::Startup, CheckTrigger::Scheduled] {
            let report = run_check_with(&state, trigger, &factory).await.unwrap();
            let nexus = report.results.iter().find(|e| e.guid == guid).unwrap();
            assert_eq!(nexus.status, UpdateState::NeedsManualCheck, "{trigger:?}");
        }
        assert_eq!(count(), 0, "an automatic check must never call the Nexus API");

        // Control: the same setup with a user-initiated check does call it,
        // so the zero above isn't just a broken mock.
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(count() >= 1);
        let nexus = report.results.iter().find(|e| e.guid == guid).unwrap();
        assert_eq!(nexus.status, UpdateState::UpdateAvailable);
        assert_eq!(nexus.latest_version.as_deref(), Some("1.2"));

        // A later automatic check keeps that result without calling Nexus.
        let before = count();
        let report = run_check_with(&state, CheckTrigger::Scheduled, &factory).await.unwrap();
        assert_eq!(count(), before);
        let nexus = report.results.iter().find(|e| e.guid == guid).unwrap();
        assert_eq!(nexus.status, UpdateState::UpdateAvailable);
    }

    #[tokio::test]
    async fn nexus_check_follows_the_plan_end_to_end() {
        let (_dir, state, guid_a) = nexus_only_state().await; // mod 1234
        let guid_b = add_nexus_mod(&state.base_path, "5678").await;
        // Recorded "updated mods" fixture, with 1234's newest file moved to
        // an hour ago; 5678 isn't in the list.
        let mut list: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/updates/nexus_updated_1w.json")).unwrap();
        list[0]["latest_file_update"] = serde_json::json!(now_unix() - 3600);
        let (base, seen) = mock_nexus(serde_json::to_string(&list).unwrap()).await;
        let factory = move |key| NexusClient::with_base(key, &base);

        // 1. Never checked: one files request per mod, no updated list.
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert_eq!(files_requests(&seen).len(), 2);
        assert!(!seen.lock().unwrap().iter().any(|p| p.contains("updated.json")));
        assert!(!report.nexus_checked_recently);

        // 2. Checked a minute ago: nothing asked, friendly note.
        seen.lock().unwrap().clear();
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(seen.lock().unwrap().is_empty(), "{:?}", seen.lock().unwrap());
        assert!(report.nexus_checked_recently);
        assert!(report.results.iter().any(|e| e.guid == guid_a && e.status == UpdateState::UpdateAvailable));

        // 3. Checked 3 days ago: one 1w list; only the listed, changed mod
        //    (1234) gets a files request.
        backdate_nexus_checks(&state, 3 * 86_400).await;
        seen.lock().unwrap().clear();
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        let paths = seen.lock().unwrap().clone();
        assert_eq!(paths.iter().filter(|p| p.contains("updated.json?period=1w")).count(), 1, "{paths:?}");
        assert_eq!(files_requests(&seen), vec!["/v1/games/helldivers2/mods/1234/files.json".to_string()]);
        assert!(!report.nexus_checked_recently);
        // 5678's earlier result still stands (and now counts as checked).
        assert!(report.results.iter().any(|e| e.guid == guid_b && e.status == UpdateState::UpdateAvailable));

        // 4. Checked 10 hours ago: the 1d window.
        backdate_nexus_checks(&state, 10 * 3600).await;
        seen.lock().unwrap().clear();
        run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(seen.lock().unwrap().iter().any(|p| p.contains("updated.json?period=1d")));

        // 5. Checked 10 days ago: the 1m window.
        backdate_nexus_checks(&state, 10 * 86_400).await;
        seen.lock().unwrap().clear();
        run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(seen.lock().unwrap().iter().any(|p| p.contains("updated.json?period=1m")));
    }

    /// Install another mod from Nexus page `mod_id`, recording which file
    /// of that page it is.
    async fn add_nexus_file_mod(base: &Path, dir_name: &str, mod_id: &str, file_name: &str) -> Uuid {
        let mod_dir = base.join("mods").join(dir_name);
        tokio::fs::create_dir_all(&mod_dir).await.unwrap();
        let guid = Uuid::new_v4();
        tokio::fs::write(
            mod_dir.join("manifest.json"),
            format!(r#"{{"Guid":"{guid}","Name":"{dir_name}","Description":"","IconPath":null,"Options":null}}"#),
        )
        .await
        .unwrap();
        sources::write_origin_sidecar_with_files(
            &mod_dir,
            vec![Source { provider: "nexus".into(), id: Some(mod_id.into()), url: None, version: Some("1.0".into()) }],
            vec![InstalledFile { provider: "nexus".into(), file_name: Some(file_name.into()), ..Default::default() }],
        )
        .await
        .unwrap();
        guid
    }

    /// A main file and an optional file of one Nexus page, installed as two
    /// mods: each keeps its own cache entry, so checking again right away
    /// asks Nexus nothing (they used to overwrite each other's entry and
    /// cost a files request every check).
    #[tokio::test]
    async fn two_files_of_one_nexus_page_keep_separate_cache_entries() {
        let (_dir, state, _guid) = nexus_only_state().await; // mod 1234
        add_nexus_file_mod(&state.base_path, "optional", "1234", "Better Stims Optional-1234-1-0-1718000001.zip").await;
        let (base, seen) = mock_nexus("[]".into()).await;
        let factory = move |key| NexusClient::with_base(key, &base);

        run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert_eq!(files_requests(&seen).len(), 1, "one request covers both files of the page");
        let cache = nexus::load_cache(&state.base_path).await;
        assert_eq!(cache.nexus.len(), 2, "{:?}", cache.nexus.keys().collect::<Vec<_>>());

        seen.lock().unwrap().clear();
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(seen.lock().unwrap().is_empty(), "{:?}", seen.lock().unwrap());
        assert!(report.nexus_checked_recently);
    }

    /// A cache written before entries were keyed per file (by mod id alone)
    /// is still used for the file it was made for, then rewritten under the
    /// new key.
    #[tokio::test]
    async fn legacy_nexus_cache_entries_are_migrated() {
        let (_dir, state, guid) = nexus_only_state().await; // mod 1234
        let installed = nexus::Installed { version: Some("1.0".into()), uploaded_at: Some(1718000000), ..Default::default() };
        let decision = nexus::Decision {
            state: nexus::DecisionState::Update,
            installed_version: Some("1.0".into()),
            latest_version: Some("9.9".into()),
            latest_file_name: None,
        };
        let mut legacy = nexus::Cache::default();
        legacy.nexus.insert(
            "1234".into(),
            nexus::CachedDecision { checked_at: now_unix(), installed_key: installed.cache_key(), decision },
        );
        // An entry for some other, no longer installed file: dropped.
        legacy.nexus.insert(
            "9999".into(),
            nexus::CachedDecision { checked_at: now_unix(), installed_key: String::new(), decision: legacy.nexus["1234"].decision.clone() },
        );
        nexus::save_cache(&state.base_path, &legacy).await;

        let (base, seen) = mock_nexus("[]".into()).await;
        let factory = move |key| NexusClient::with_base(key, &base);
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        assert!(seen.lock().unwrap().is_empty(), "the migrated entry is still fresh: {:?}", seen.lock().unwrap());
        let e = report.results.iter().find(|e| e.guid == guid).unwrap();
        assert_eq!(e.latest_version.as_deref(), Some("9.9"));

        let cache = nexus::load_cache(&state.base_path).await;
        let keys: Vec<_> = cache.nexus.keys().cloned().collect();
        assert_eq!(keys, vec![nexus::cache_entry_key("1234", &installed.cache_key())]);
    }

    #[tokio::test]
    async fn a_skipped_version_stays_skipped_when_the_site_adds_a_v() {
        let (_dir, state, guid) = nexus_only_state().await; // latest on the mock: 1.2
        sources::set_skipped_version(&state.base_path.join("mods").join("nexus-mod"), "nexus", Some("v1.2"), None)
            .await
            .unwrap();
        let (base, _seen) = mock_nexus("[]".into()).await;
        let factory = move |key| NexusClient::with_base(key, &base);
        let report = run_check_with(&state, CheckTrigger::Manual, &factory).await.unwrap();
        let e = report.results.iter().find(|e| e.guid == guid).unwrap();
        assert_eq!(e.status, UpdateState::Skipped, "{e:?}");
    }

    #[test]
    fn only_manual_checks_are_user_initiated() {
        assert!(CheckTrigger::Manual.is_user_initiated());
        assert!(!CheckTrigger::Startup.is_user_initiated());
        assert!(!CheckTrigger::Scheduled.is_user_initiated());
    }

    #[test]
    fn report_counts_mods_not_sources() {
        let g = Uuid::new_v4();
        let mk = |status| UpdateStatusEntry {
            guid: g,
            provider: "github".into(),
            display_name: "GitHub".into(),
            source_id: None,
            installed_version: None,
            latest_version: None,
            latest_file_name: None,
            latest_modified_at: None,
            status,
            page_url: None,
            method: None,
            files: vec![],
            preselected_file: None,
        };
        let report = UpdateCheckReport {
            trigger: CheckTrigger::Manual,
            checked_at: 0,
            results: vec![mk(UpdateState::UpdateAvailable), mk(UpdateState::UpdateAvailable), mk(UpdateState::Skipped)],
            nexus_rate_limit: None,
            nexus_checked_recently: false,
        };
        assert_eq!(report.available_count(), 1);
    }
}
