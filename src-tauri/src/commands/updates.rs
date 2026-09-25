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
    skipped_version: Option<String>,
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
            let skipped_version = sidecar.as_ref().and_then(|s| {
                s.skipped_versions
                    .iter()
                    .find(|v| v.provider.eq_ignore_ascii_case(&provider))
                    .map(|v| v.version.clone())
            });

            targets.push(Target {
                guid: m.guid(),
                provider,
                id,
                display_name: source.display_name.clone(),
                page_url: source.page_url.clone(),
                installed_version,
                installed_file,
                skipped_version,
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

fn entry(t: &Target, status: UpdateState, latest: Option<String>) -> UpdateStatusEntry {
    UpdateStatusEntry {
        guid: t.guid,
        provider: t.provider.clone(),
        display_name: t.display_name.clone(),
        source_id: Some(t.id.clone()),
        installed_version: t.installed_version.clone(),
        latest_version: latest,
        latest_file_name: None,
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
        "ayakamods" => {
            pacer.wait(ayakamods::HOST, SAME_HOST_DELAY).await;
            match ayakamods::fetch_ayakamods_metadata(client, &t.id).await {
                Ok(Some(meta)) => {
                    let status = status_for(t.installed_version.as_deref(), meta.latest_version.as_deref());
                    let mut e = entry(t, status, meta.latest_version);
                    e.method = Some(UpdateMethod::Browser);
                    e
                }
                Ok(None) => entry(t, UpdateState::Unknown, None),
                Err(err) => error_entry(t, err),
            }
        }
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

    // What we know about each target: its id, installed file, and when that
    // file was last checked (a cache entry for a different installed file
    // doesn't count).
    let info: Vec<Option<(u64, nexus::Installed, String, Option<i64>)>> = targets
        .iter()
        .map(|t| {
            let mod_id = t.id.parse::<u64>().ok()?;
            let installed = nexus_installed(t);
            let key = installed.cache_key();
            let last = cache.nexus.get(&t.id).filter(|c| c.installed_key == key).map(|c| c.checked_at);
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
                    } else if let Some(c) = cache.nexus.get_mut(&mod_id.to_string()) {
                        // Nothing new since the last check: that result
                        // still holds, as of now.
                        if &c.installed_key == key {
                            c.checked_at = now;
                        }
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
            match cache.nexus.get(&t.id).filter(|c| &c.installed_key == key) {
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
                    t.id.clone(),
                    nexus::CachedDecision { checked_at: now, installed_key: key.clone(), decision: decision.clone() },
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

    for (i, t) in targets.iter().enumerate().filter(|(_, t)| t.provider != "nexus") {
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
    for (t, e) in targets.iter().zip(by_index) {
        let mut e = e.unwrap_or_else(|| entry(t, UpdateState::Unknown, None));
        if e.method == Some(UpdateMethod::Direct) {
            e.preselected_file = preselect(&e.files, t.installed_file.as_ref());
        }
        if e.status == UpdateState::UpdateAvailable {
            if let (Some(skipped), Some(latest)) = (&t.skipped_version, &e.latest_version) {
                if skipped == latest {
                    e.status = UpdateState::Skipped;
                }
            }
        } else {
            e.method = None;
            e.files.clear();
            e.preselected_file = None;
        }
        results.push(e);
    }

    let report = UpdateCheckReport {
        trigger,
        checked_at: now_unix(),
        results,
        nexus_rate_limit: nexus_rate,
        nexus_checked_recently: nexus_all_recent,
    };
    log::info!(
        "Update check done: {} source(s) checked, {} mod(s) with updates.",
        report.results.len(),
        report.available_count()
    );
    *state.last_update_report.lock().await = Some(report.clone());
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

/// "Skip this version" (`version: Some`) / "Stop skipping" (`None`).
#[tauri::command]
pub async fn skip_update_version(
    state: State<'_, AppState>,
    guid: Uuid,
    provider: String,
    version: Option<String>,
) -> TAResult<()> {
    let dir = {
        let guard = state.mods.lock().await;
        guard
            .as_ref()
            .and_then(|mods| mods.iter().find(|m| m.guid() == guid))
            .map(|m| m.directory.clone())
    }
    .ok_or_else(|| anyhow::anyhow!("mod {{{guid}}} not found"))
    .into_ta_result()?;
    sources::set_skipped_version(&dir, &provider, version.as_deref()).await.into_ta_result()?;

    if let Some(report) = state.last_update_report.lock().await.as_mut() {
        for e in report.results.iter_mut().filter(|e| e.guid == guid && e.provider.eq_ignore_ascii_case(&provider)) {
            match (&version, &e.status) {
                (Some(v), UpdateState::UpdateAvailable) if e.latest_version.as_deref() == Some(v.as_str()) => {
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
/// call); the keyless sites are asked for their current version. Never
/// fails an install.
pub async fn enrich_install_source(source: &Source, archive_path: Option<&Path>) -> (Source, Vec<InstalledFile>) {
    let mut source = source.clone();
    let mut files = Vec::new();
    let provider = source.provider.to_ascii_lowercase();
    let file_name = archive_path.and_then(|p| p.file_name()).and_then(|n| n.to_str()).map(str::to_string);

    if provider == "nexus" {
        if let Some(name) = &file_name {
            if let Some(parsed) = sources::parse_nexus_archive_name(name) {
                if source.id.as_deref().is_none_or(|id| id == parsed.mod_id) {
                    if source.version.is_none() {
                        source.version = Some(parsed.version.clone());
                    }
                    files.push(InstalledFile {
                        provider: "nexus".into(),
                        file_id: None,
                        file_name: Some(name.clone()),
                        label: None,
                        uploaded_at: Some(parsed.uploaded_at),
                    });
                }
            }
        }
        return (source, files);
    }

    if let Some(name) = file_name {
        files.push(InstalledFile { provider: provider.clone(), file_name: Some(name), ..Default::default() });
    }
    if source.version.is_some() {
        return (source, files);
    }
    let Some(id) = source.id.clone() else { return (source, files) };
    let Ok(client) = providers::build_client() else { return (source, files) };
    source.version = match provider.as_str() {
        "ayakamods" => ayakamods::fetch_ayakamods_metadata(&client, &id).await.ok().flatten().and_then(|m| m.latest_version),
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
    let provider = provider.to_ascii_lowercase();
    if !DIRECT_PROVIDERS.contains(&provider.as_str()) {
        return anyhow::anyhow!("{provider} updates go through the browser").into_ta_result();
    }
    if !providers::is_allowed_download_host(&provider, &file.url) {
        return anyhow::anyhow!("refusing to download an update for a {provider} mod from {}", crate::download::redact_url(&file.url))
            .into_ta_result();
    }

    // Everything needed to rewrite the sidecar, captured before the old
    // mod directory (which holds the sidecar) is swapped out.
    let (mod_dir, source_id, old_sidecar) = {
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
        (m.directory.clone(), id, sources::load_origin_sidecar(&m.directory).await)
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

    let result = {
        let mut guard = state.mods.lock().await;
        match guard.as_mut() {
            Some(mods) => install_update_from_archive(&state, mods, &downloaded.path, guid).await,
            None => anyhow::anyhow!("mods not read").into_ta_result(),
        }
    };
    let _ = tokio::fs::remove_dir_all(&downloaded.temp_dir).await;
    let (mut r#mod, warning) = result?;

    // Record where it came from and exactly what was installed.
    let mut recorded_sources: Vec<Source> = old_sidecar.as_ref().map(|s| s.sources.clone()).unwrap_or_default();
    recorded_sources.retain(|s| !s.provider.eq_ignore_ascii_case(&provider));
    recorded_sources.push(Source { provider: provider.clone(), id: Some(source_id), url: None, version: version.clone() });
    let mut installed_files: Vec<InstalledFile> = old_sidecar.map(|s| s.installed_files).unwrap_or_default();
    installed_files.retain(|f| !f.provider.eq_ignore_ascii_case(&provider));
    installed_files.push(InstalledFile {
        provider: provider.clone(),
        file_id: Some(file.id.clone()),
        file_name: Some(file.name.clone()),
        label: file.label.clone(),
        uploaded_at: file.uploaded_at,
    });
    if let Err(e) = sources::write_origin_sidecar_with_files(&r#mod.directory, recorded_sources, installed_files).await {
        log::error!("Failed to write origin sidecar after update: {e}");
    }
    r#mod.resolve_sources().await;
    {
        let mut guard = state.mods.lock().await;
        if let Some(mods) = guard.as_mut() {
            if let Some(existing) = mods.iter_mut().find(|m| m.guid() == r#mod.guid()) {
                *existing = r#mod.clone();
            }
        }
    }
    debug_assert_eq!(r#mod.directory, mod_dir);

    mark_mod_updated(&state, guid, r#mod.guid(), Some(&provider), version.as_deref()).await;
    log::info!("Mod {{{}}} updated from {provider} to {:?}.", r#mod.guid(), version);
    Ok(InstalledMod { r#mod, warning })
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
            file_name: Some("rabu_ss_sa-7_no_helm_00aa1.zip".into()),
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

        sources::set_skipped_version(dir.path(), "gamebanana", Some("1.2")).await.unwrap();
        let targets = collect_targets(&[m]).await;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "5");
        assert_eq!(targets[0].installed_version.as_deref(), Some("1.1"));
        assert_eq!(targets[0].skipped_version.as_deref(), Some("1.2"));
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
            skipped_version: None,
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
        let (s, files) = enrich_install_source(&source, Some(Path::new("/dl/Better Stims-1234-1-1-1718100000.zip"))).await;
        assert_eq!(s.version.as_deref(), Some("1.1"));
        assert_eq!(files[0].uploaded_at, Some(1718100000));

        // A file from a *different* Nexus mod never stamps this one.
        let (s, files) = enrich_install_source(&source, Some(Path::new("/dl/Other-99-2-0-1718100000.zip"))).await;
        assert_eq!(s.version, None);
        assert!(files.is_empty());
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
