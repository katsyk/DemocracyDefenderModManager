//! Browser handoff: for mod-page URLs the manager can't download directly
//! (a login-gated site like AyakaMods or Nexus Mods), open the page in the
//! user's browser and watch the Downloads folder for the archive to land,
//! then install it exactly as if it had been picked by hand.
//!
//! Only one handoff runs at a time. Progress is reported to the frontend
//! via a `handoff` Tauri event rather than a long-running command, so the
//! command itself returns as soon as the background watch starts.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use anyhow_tauri::{IntoTAResult, TAResult};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::{
    commands::{
        mods::{install_from_archive, install_update_from_archive},
        settings::do_load_settings,
    },
    models::{manifest::Source, Mod},
    sources, AppState,
};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const STABLE_POLLS_REQUIRED: u32 = 2;
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Reused by `auto_import`, which watches the same folder for the same
/// reason (a still-downloading file shouldn't be treated as a candidate).
pub(crate) const IGNORED_SUFFIXES: [&str; 5] = [".crdownload", ".part", ".tmp", ".download", ".partial"];
pub(crate) const ARCHIVE_SUFFIXES: [&str; 3] = [".zip", ".7z", ".rar"];

/// The single `handoff` event the frontend listens for; `status` picks
/// which of the optional fields are meaningful.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HandoffEvent {
    pub status: HandoffStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#mod: Option<Mod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum HandoffStatus {
    /// Watching the downloads folder.
    Waiting,
    /// A candidate file was found and is being installed.
    Installing,
    /// Installed successfully -- `r#mod` (and maybe `warning`) are set.
    Done,
    /// The user cancelled via `cancel_handoff`.
    Cancelled,
    /// 15 minutes passed with no matching file appearing.
    TimedOut,
    /// Something went wrong -- see `message`.
    Error,
}

const HANDOFF_EVENT: &str = "handoff";

fn emit(app: &AppHandle, event: HandoffEvent) {
    if let Err(e) = app.emit(HANDOFF_EVENT, event) {
        log::error!("Failed to emit handoff event: {}", e);
    }
}

/// Sites this manager knows it can never download from directly (they
/// gate the actual file behind a login), so a pasted mod-page URL for one
/// of them should go straight to a handoff instead of attempting -- and
/// failing -- a direct download first.
const LOGIN_GATED_PROVIDERS: [&str; 2] = ["ayakamods", "nexus"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UrlClassification {
    pub provider: String,
    pub display_name: String,
    pub requires_handoff: bool,
}

/// Classify a pasted URL before deciding whether to try a direct download
/// or go straight to a browser handoff. Never touches the network itself.
#[tauri::command]
pub fn classify_download_url(url: String) -> UrlClassification {
    match sources::source_from_page_url(&url) {
        Some(source) => {
            let requires_handoff = LOGIN_GATED_PROVIDERS.contains(&source.provider.as_str());
            let resolved = sources::resolve(&source);
            UrlClassification {
                provider: source.provider,
                display_name: resolved.display_name,
                requires_handoff,
            }
        }
        None => {
            let provider = sources::provider_from_url(&url);
            let resolved = sources::resolve(&Source {
                provider: provider.clone(),
                id: None,
                url: Some(url),
                version: None,
            });
            UrlClassification {
                provider,
                display_name: resolved.display_name,
                requires_handoff: false,
            }
        }
    }
}

/// Start watching the configured Downloads folder for an archive to
/// install, after opening `page_url` for the user in their browser (the
/// frontend does the actual opening via the opener plugin). If
/// `existing_guid` is set, a successfully-found archive updates that mod in
/// place instead of being installed as a new one.
///
/// Returns as soon as the watch has started; progress comes through the
/// `handoff` event.
#[tauri::command]
pub async fn start_handoff(
    app: AppHandle,
    state: State<'_, AppState>,
    page_url: String,
    existing_guid: Option<Uuid>,
) -> TAResult<()> {
    let _data_op = state.data_op().into_ta_result()?;
    let cancel = {
        let mut guard = state.handoff_cancel.lock().await;
        if guard.is_some() {
            return anyhow::anyhow!("a handoff is already in progress").into_ta_result();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *guard = Some(cancel.clone());
        cancel
    };

    let settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    let downloads_dir = settings.downloads_path().to_path_buf();

    if downloads_dir.as_os_str().is_empty() || !downloads_dir.is_dir() {
        let mut guard = state.handoff_cancel.lock().await;
        *guard = None;
        return anyhow::anyhow!(
            "the downloads folder isn't set or doesn't exist -- check it in Settings"
        )
        .into_ta_result();
    }

    if let Err(e) = crate::commands::settings::check_downloads_path(&state.base_path, &downloads_dir) {
        let mut guard = state.handoff_cancel.lock().await;
        *guard = None;
        return Err(e).into_ta_result();
    }

    let source = sources::source_from_page_url(&page_url).unwrap_or_else(|| Source {
        provider: sources::provider_from_url(&page_url),
        id: None,
        url: Some(page_url.clone()),
        version: None,
    });

    log::info!(
        "Starting handoff for {} (provider {:?})...",
        page_url,
        source.provider
    );

    tokio::spawn(run_handoff(
        app,
        downloads_dir,
        source,
        existing_guid,
        cancel,
    ));

    Ok(())
}

/// Cancel the in-flight handoff, if any. A no-op (not an error) if none is
/// running.
#[tauri::command]
pub async fn cancel_handoff(state: State<'_, AppState>) -> TAResult<()> {
    let guard = state.handoff_cancel.lock().await;
    if let Some(cancel) = guard.as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Install a file the user says they already downloaded by hand (the "I
/// already downloaded it -- choose file" fallback), bypassing the folder
/// watch entirely but going through the same install/update path and
/// recording the same origin `Source`.
#[tauri::command]
pub async fn install_handoff_file(
    state: State<'_, AppState>,
    file: PathBuf,
    page_url: String,
    existing_guid: Option<Uuid>,
) -> TAResult<crate::commands::mods::InstalledMod> {
    let _data_op = state.data_op().into_ta_result()?;
    let source = sources::source_from_page_url(&page_url).unwrap_or_else(|| Source {
        provider: sources::provider_from_url(&page_url),
        id: None,
        url: Some(page_url.clone()),
        version: None,
    });

    let (r#mod, warning) = install_found_file(&state, &file, &source, existing_guid)
        .await
        .into_ta_result()?;

    Ok(crate::commands::mods::InstalledMod { r#mod, warning })
}

async fn run_handoff(
    app: AppHandle,
    downloads_dir: PathBuf,
    source: Source,
    existing_guid: Option<Uuid>,
    cancel: Arc<AtomicBool>,
) {
    let outcome = wait_for_download(&app, &downloads_dir, &cancel).await;

    match outcome {
        Ok(WaitOutcome::Found(path)) => {
            emit(
                &app,
                HandoffEvent {
                    status: HandoffStatus::Installing,
                    r#mod: None,
                    warning: None,
                    message: None,
                },
            );

            let state = app.state::<AppState>();
            match install_found_file(&state, &path, &source, existing_guid).await {
                Ok((r#mod, warning)) => emit(
                    &app,
                    HandoffEvent {
                        status: HandoffStatus::Done,
                        r#mod: Some(r#mod),
                        warning,
                        message: None,
                    },
                ),
                Err(e) => emit(
                    &app,
                    HandoffEvent {
                        status: HandoffStatus::Error,
                        r#mod: None,
                        warning: None,
                        message: Some(e.to_string()),
                    },
                ),
            }
        }
        Ok(WaitOutcome::Cancelled) => emit(
            &app,
            HandoffEvent {
                status: HandoffStatus::Cancelled,
                r#mod: None,
                warning: None,
                message: None,
            },
        ),
        Ok(WaitOutcome::TimedOut) => emit(
            &app,
            HandoffEvent {
                status: HandoffStatus::TimedOut,
                r#mod: None,
                warning: None,
                message: None,
            },
        ),
        Err(e) => emit(
            &app,
            HandoffEvent {
                status: HandoffStatus::Error,
                r#mod: None,
                warning: None,
                message: Some(e.to_string()),
            },
        ),
    }

    let state = app.state::<AppState>();
    let mut guard = state.handoff_cancel.lock().await;
    *guard = None;
}

async fn install_found_file(
    state: &AppState,
    archive_path: &Path,
    source: &Source,
    existing_guid: Option<Uuid>,
) -> anyhow::Result<(Mod, Option<String>)> {
    // Done before taking the mods lock so a slow/failed metadata fetch
    // never holds up anything else touching the mod list. Best-effort:
    // falls back to the plain source (no Version) on any failure.
    let (mut sidecar_source, installed_files) =
        crate::commands::updates::enrich_install_source(source, Some(archive_path)).await;
    // An update the last check found, arriving without a version of its
    // own: record the version that check reported.
    if sidecar_source.version.is_none() {
        if let Some(guid) = existing_guid {
            sidecar_source.version =
                crate::commands::updates::known_latest_version(state, guid, &sidecar_source.provider).await;
        }
    }

    let (r#mod, warning) = {
        let mut mods_guard = state.mods.lock().await;
        let mods = mods_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("mods not read"))?;

        let (mut r#mod, warning) = match existing_guid {
            Some(guid) => install_update_from_archive(state, mods, archive_path, guid)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            None => install_from_archive(state, mods, archive_path)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        };

        if let Err(e) =
            sources::write_origin_sidecar_with_files(&r#mod.directory, vec![sidecar_source.clone()], installed_files).await
        {
            log::error!("Failed to write origin sidecar: {}", e);
        }
        r#mod.resolve_sources().await;

        if let Some(existing) = mods.iter_mut().find(|m| m.guid() == r#mod.guid()) {
            *existing = r#mod.clone();
        }
        (r#mod, warning)
    };

    if let Some(guid) = existing_guid {
        crate::commands::updates::mark_mod_updated(
            state,
            guid,
            r#mod.guid(),
            Some(&sidecar_source.provider),
            sidecar_source.version.as_deref(),
        )
        .await;
    }

    Ok((r#mod, warning))
}

enum WaitOutcome {
    Found(PathBuf),
    Cancelled,
    TimedOut,
}

async fn wait_for_download(
    app: &AppHandle,
    downloads_dir: &Path,
    cancel: &AtomicBool,
) -> anyhow::Result<WaitOutcome> {
    let start = SystemTime::now();
    let existing_before = snapshot_names(downloads_dir).await?;
    let deadline = start + HANDOFF_TIMEOUT;

    emit(
        app,
        HandoffEvent {
            status: HandoffStatus::Waiting,
            r#mod: None,
            warning: None,
            message: None,
        },
    );

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(WaitOutcome::Cancelled);
        }
        if SystemTime::now() >= deadline {
            return Ok(WaitOutcome::TimedOut);
        }

        tokio::time::sleep(POLL_INTERVAL).await;

        if cancel.load(Ordering::Relaxed) {
            return Ok(WaitOutcome::Cancelled);
        }

        if let Some(candidate) = find_new_archive(downloads_dir, &existing_before, start).await? {
            match wait_until_stable(&candidate, cancel).await? {
                Some(path) => return Ok(WaitOutcome::Found(path)),
                None => return Ok(WaitOutcome::Cancelled),
            }
        }
    }
}

async fn snapshot_names(dir: &Path) -> anyhow::Result<HashSet<String>> {
    let mut names = HashSet::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if let Some(name) = entry.file_name().to_str() {
            names.insert(name.to_string());
        }
    }
    Ok(names)
}

/// Find a file in `dir` that is: not already present before the handoff
/// started, not one of the known "still downloading" temp names, has a
/// supported archive extension, was modified after `start`, and (checked
/// with `symlink_metadata`, never followed) is a plain file, not a symlink.
async fn find_new_archive(
    dir: &Path,
    existing_before: &HashSet<String>,
    start: SystemTime,
) -> anyhow::Result<Option<PathBuf>> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();

        if IGNORED_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
            continue;
        }
        if !ARCHIVE_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
            continue;
        }
        if existing_before.contains(&name) {
            continue;
        }

        let path = entry.path();
        let meta = match tokio::fs::symlink_metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let modified = match meta.modified() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if modified <= start {
            continue;
        }

        return Ok(Some(path));
    }
    Ok(None)
}

/// Poll a candidate file's size until it hasn't changed for
/// [`STABLE_POLLS_REQUIRED`] consecutive checks (i.e. the browser has
/// finished writing it). Returns `Ok(None)` if cancelled mid-wait.
async fn wait_until_stable(path: &Path, cancel: &AtomicBool) -> anyhow::Result<Option<PathBuf>> {
    let mut last_size: Option<u64> = None;
    let mut stable_count = 0u32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let meta = tokio::fs::symlink_metadata(path).await?;
        if meta.file_type().is_symlink() {
            anyhow::bail!("refusing to install a symlink");
        }
        if !meta.is_file() {
            // Disappeared or changed type mid-download; give up on this candidate.
            return Ok(None);
        }
        let size = meta.len();

        if Some(size) == last_size {
            stable_count += 1;
            if stable_count >= STABLE_POLLS_REQUIRED {
                return Ok(Some(path.to_path_buf()));
            }
        } else {
            stable_count = 0;
            last_size = Some(size);
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn touch(path: &Path, contents: &[u8]) {
        tokio::fs::write(path, contents).await.unwrap();
    }

    #[tokio::test]
    async fn find_new_archive_ignores_temp_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        let start = SystemTime::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        touch(&dir.path().join("mod.zip.crdownload"), b"partial").await;
        touch(&dir.path().join("mod.part"), b"partial").await;

        let found = find_new_archive(dir.path(), &HashSet::new(), start)
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_new_archive_ignores_pre_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("old.zip"), b"already here").await;
        let existing = snapshot_names(dir.path()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;
        let start = SystemTime::now();
        tokio::time::sleep(Duration::from_millis(10)).await;

        let found = find_new_archive(dir.path(), &existing, start).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_new_archive_finds_a_new_supported_archive() {
        let dir = tempfile::tempdir().unwrap();
        let existing = snapshot_names(dir.path()).await.unwrap();

        let start = SystemTime::now();
        tokio::time::sleep(Duration::from_millis(20)).await;
        touch(&dir.path().join("cool-mod.zip"), b"pkzip data").await;

        let found = find_new_archive(dir.path(), &existing, start).await.unwrap();
        assert_eq!(found, Some(dir.path().join("cool-mod.zip")));
    }

    #[tokio::test]
    async fn find_new_archive_ignores_non_archive_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let existing = snapshot_names(dir.path()).await.unwrap();

        let start = SystemTime::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        touch(&dir.path().join("readme.txt"), b"not an archive").await;

        let found = find_new_archive(dir.path(), &existing, start).await.unwrap();
        assert!(found.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn find_new_archive_ignores_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        touch(&outside.path().join("real.zip"), b"pkzip data").await;
        let existing = snapshot_names(dir.path()).await.unwrap();

        let start = SystemTime::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        tokio::fs::symlink(outside.path().join("real.zip"), dir.path().join("evil.zip"))
            .await
            .unwrap();

        let found = find_new_archive(dir.path(), &existing, start).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn wait_until_stable_returns_none_when_already_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mod.zip");
        touch(&path, b"data").await;

        let cancel = AtomicBool::new(true);
        let result = wait_until_stable(&path, &cancel).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn classify_ayakamods_page_requires_handoff() {
        let c = classify_download_url("https://ayakamods.com/mods/hd2-auto-reload.4084/".to_string());
        assert_eq!(c.provider, "ayakamods");
        assert!(c.requires_handoff);
    }

    #[test]
    fn classify_nexus_page_requires_handoff() {
        let c = classify_download_url(
            "https://www.nexusmods.com/helldivers2/mods/123".to_string(),
        );
        assert_eq!(c.provider, "nexus");
        assert!(c.requires_handoff);
    }

    #[test]
    fn classify_github_page_does_not_require_handoff() {
        let c = classify_download_url("https://github.com/someone/example-mod".to_string());
        assert_eq!(c.provider, "github");
        assert!(!c.requires_handoff);
    }

    #[test]
    fn classify_direct_link_does_not_require_handoff() {
        let c = classify_download_url("https://example.com/downloads/mod.zip".to_string());
        assert_eq!(c.provider, "url");
        assert!(!c.requires_handoff);
    }
}
