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
    install_error::{InstallError, InstallStep},
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

/// How long a `<name>.part` / `.crdownload` / ... next to a download may
/// go unchanged and still count as "being written". Older than this it is
/// a leftover (a paused, crashed or abandoned download) and is ignored.
pub(crate) const PARTIAL_STALE_AFTER: Duration = Duration::from_secs(60);

/// Whether a browser is still writing `path`: a "still downloading" file
/// for it (`<name>.part`, `<name>.crdownload`, ...) sits next to it and
/// was written to within [`PARTIAL_STALE_AFTER`]. Firefox reserves the
/// final name with an empty file as soon as a download starts and writes
/// into `<name>.part` until it's done, so the empty file alone means
/// nothing; picking it up would try to install an empty archive.
///
/// A file with no active partial next to it is final, even if it is
/// empty (a download that really is 0 bytes): the install then says so.
pub(crate) async fn download_in_progress(path: &Path) -> bool {
    active_partial(path, PARTIAL_STALE_AFTER).await.is_some()
}

/// The actively written partial-download file next to `path`, if any.
async fn active_partial(path: &Path, stale_after: Duration) -> Option<PathBuf> {
    let name = path.file_name()?;
    for suffix in IGNORED_SUFFIXES {
        let mut sibling = name.to_os_string();
        sibling.push(suffix);
        let sibling = path.with_file_name(sibling);
        let Ok(meta) = tokio::fs::symlink_metadata(&sibling).await else { continue };
        let fresh = match meta.modified().ok().map(|m| SystemTime::now().duration_since(m)) {
            // Modified in the future (clock skew) counts as fresh.
            Some(Ok(age)) => age <= stale_after,
            Some(Err(_)) => true,
            None => true,
        };
        if fresh {
            return Some(sibling);
        }
    }
    None
}

/// Limits for [`wait_until_stable`] (tests use short ones).
#[derive(Debug, Clone, Copy)]
struct WaitLimits {
    deadline: SystemTime,
    stale_after: Duration,
}

fn file_label(path: &Path) -> String {
    crate::install_error::subject_of(path)
}

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
        crate::download::redact_url(&page_url),
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
    subject: String,
    source: Source,
    existing_guid: Option<Uuid>,
    cancel: Arc<AtomicBool>,
) {
    let outcome = wait_for_download(&app, &downloads_dir, &subject, &cancel).await;

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
        Err(e) => {
            log::error!("{e}");
            emit(
                &app,
                HandoffEvent {
                    status: HandoffStatus::Error,
                    r#mod: None,
                    warning: None,
                    message: Some(e.to_string()),
                },
            )
        }
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
    subject: &str,
    cancel: &AtomicBool,
) -> Result<WaitOutcome, InstallError> {
    let start = SystemTime::now();
    let existing_before = snapshot_names(downloads_dir)
        .await
        .map_err(|e| downloads_folder_error(subject, downloads_dir, e))?;
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

        let found = find_new_archive(downloads_dir, &existing_before, start)
            .await
            .map_err(|e| downloads_folder_error(subject, downloads_dir, e))?;
        if let Some(candidate) = found {
            let limits = WaitLimits { deadline, stale_after: PARTIAL_STALE_AFTER };
            match wait_until_stable(&candidate, cancel, limits).await? {
                Some(path) => return Ok(WaitOutcome::Found(path)),
                None => return Ok(WaitOutcome::Cancelled),
            }
        }
    }
}

async fn snapshot_names(dir: &Path) -> std::io::Result<HashSet<String>> {
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
) -> std::io::Result<Option<PathBuf>> {
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
/// [`STABLE_POLLS_REQUIRED`] consecutive checks and no partial-download
/// file next to it is still being written (i.e. the browser has finished
/// it). Returns `Ok(None)` if cancelled mid-wait, and an [`InstallError`]
/// saying what was still going on if `limits.deadline` passes first, or if
/// the file disappears or turns out to be a symlink.
async fn wait_until_stable(path: &Path, cancel: &AtomicBool, limits: WaitLimits) -> Result<Option<PathBuf>, InstallError> {
    let subject = file_label(path);
    let mut last_size: Option<u64> = None;
    let mut stable_count = 0u32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let meta = match tokio::fs::symlink_metadata(path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(vanished(&subject)),
            Err(e) => {
                return Err(InstallError::new(
                    subject,
                    InstallStep::Download,
                    crate::archive::io_error(e, "checking the downloaded file failed"),
                ))
            }
        };
        if meta.file_type().is_symlink() {
            return Err(InstallError::new(
                subject,
                InstallStep::Download,
                anyhow::anyhow!("the file in the Downloads folder is a symbolic link, and DDMM never installs through one"),
            )
            .with_hint("Download the mod again so the real file lands in the Downloads folder, or add the file it points to with Add."));
        }
        if !meta.is_file() {
            return Err(vanished(&subject));
        }
        let size = meta.len();
        let partial = active_partial(path, limits.stale_after).await;

        if SystemTime::now() >= limits.deadline {
            return Err(still_unfinished(&subject, size, partial.as_deref()));
        }

        if partial.is_some() {
            // Not finished, however long its size stays the same.
            stable_count = 0;
            last_size = None;
        } else if Some(size) == last_size {
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

const HINT_CHOOSE_FILE: &str = "Once the browser has finished the download, use \"I already downloaded it -- choose \
     file\", or add the file with Add.";

fn vanished(subject: &str) -> InstallError {
    InstallError::new(
        subject,
        InstallStep::Download,
        anyhow::anyhow!(
            "the file disappeared from the Downloads folder before it was finished (it was moved, renamed or \
             deleted, or the browser cancelled the download)"
        ),
    )
    .with_hint(HINT_CHOOSE_FILE)
}

/// The deadline passed while `subject` was still not a finished download.
fn still_unfinished(subject: &str, size: u64, partial: Option<&Path>) -> InstallError {
    let minutes = HANDOFF_TIMEOUT.as_secs() / 60;
    let cause = match partial {
        Some(partial) if size == 0 => anyhow::anyhow!(
            "the download was still in progress after {minutes} minutes: the file is still empty and the browser \
             is still writing \"{}\"",
            file_label(partial)
        ),
        Some(partial) => anyhow::anyhow!(
            "the download was still in progress after {minutes} minutes: the browser is still writing \"{}\"",
            file_label(partial)
        ),
        None if size == 0 => anyhow::anyhow!("the file was still empty (0 bytes) after {minutes} minutes"),
        None => anyhow::anyhow!(
            "the file was still growing after {minutes} minutes ({size} bytes so far), so the download never finished"
        ),
    };
    InstallError::new(subject, InstallStep::Download, cause).with_hint(HINT_CHOOSE_FILE)
}

/// A failure reading the Downloads folder itself.
fn downloads_folder_error(subject: &str, dir: &Path, e: std::io::Error) -> InstallError {
    InstallError::new(
        subject,
        InstallStep::Download,
        crate::archive::io_error(e, &format!("reading the Downloads folder ({}) failed", dir.display())),
    )
    .with_hint("Check that the Downloads folder in Settings exists and DDMM can open it, then try again.")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn touch(path: &Path, contents: &[u8]) {
        tokio::fs::write(path, contents).await.unwrap();
    }

    /// Firefox reserves `mod.zip` with an empty file the moment a download
    /// starts and writes into `mod.zip.part`; neither is finished.
    #[tokio::test]
    async fn empty_placeholders_and_files_with_a_part_are_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koyuki launcher.zip");
        touch(&path, b"").await;
        touch(&dir.path().join("koyuki launcher.zip.part"), b"PK\x03\x04half").await;
        assert!(download_in_progress(&path).await, "empty placeholder");

        touch(&path, b"PK\x03\x04full").await;
        assert!(download_in_progress(&path).await, "its .part is still there");

        tokio::fs::remove_file(dir.path().join("koyuki launcher.zip.part")).await.unwrap();
        assert!(!download_in_progress(&path).await);

        // Empty with nothing being written next to it: a finished 0-byte
        // download, left for the install to report.
        touch(&path, b"").await;
        assert!(!download_in_progress(&path).await);

        let chrome = dir.path().join("yuuka hammer.rar");
        touch(&chrome, b"Rar!\x1a\x07\x01\x00").await;
        touch(&dir.path().join("yuuka hammer.rar.crdownload"), b"x").await;
        assert!(download_in_progress(&chrome).await);
    }

    fn long_limits() -> WaitLimits {
        WaitLimits { deadline: SystemTime::now() + Duration::from_secs(600), stale_after: PARTIAL_STALE_AFTER }
    }

    fn short_limits(secs: u64) -> WaitLimits {
        WaitLimits { deadline: SystemTime::now() + Duration::from_secs(secs), stale_after: Duration::from_secs(30) }
    }

    fn age(path: &Path, by: Duration) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - by).unwrap();
    }

    /// A download that really is 0 bytes (Chrome, finished, nothing next to
    /// it) is final: it is handed to the install, which says the file is
    /// empty, instead of being waited on forever.
    #[tokio::test]
    async fn a_finished_empty_download_is_not_waited_on_forever() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mod.zip");
        touch(&path, b"").await;
        let found = tokio::time::timeout(POLL_INTERVAL * 6, wait_until_stable(&path, &AtomicBool::new(false), short_limits(60)))
            .await
            .expect("must not hang")
            .unwrap();
        assert_eq!(found.as_deref(), Some(path.as_path()));
    }

    /// A stale `mod.zip.part` / `.crdownload` from an earlier download next
    /// to a later, complete `mod.zip` doesn't block it.
    #[tokio::test]
    async fn a_stale_partial_next_to_a_complete_file_is_ignored() {
        for suffix in [".part", ".crdownload"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("mod.zip");
            let partial = dir.path().join(format!("mod.zip{suffix}"));
            touch(&partial, b"old").await;
            age(&partial, Duration::from_secs(120));
            touch(&path, b"PK\x03\x04complete").await;
            assert!(!download_in_progress(&path).await, "{suffix}");
            let found = tokio::time::timeout(
                POLL_INTERVAL * 6,
                wait_until_stable(&path, &AtomicBool::new(false), short_limits(60)),
            )
            .await
            .expect("must not hang")
            .unwrap();
            assert_eq!(found.as_deref(), Some(path.as_path()), "{suffix}");
        }
    }

    /// A paused Firefox download: an empty placeholder next to a `.part`
    /// that keeps being "fresh" (or a download that simply never ends)
    /// stops at the deadline with a specific error, not a hang.
    #[tokio::test]
    async fn a_download_still_in_progress_stops_at_the_deadline_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koyuki launcher.zip");
        touch(&path, b"").await;
        touch(&dir.path().join("koyuki launcher.zip.part"), b"PK\x03\x04half").await;
        let err = tokio::time::timeout(
            POLL_INTERVAL * 8,
            wait_until_stable(&path, &AtomicBool::new(false), short_limits(2)),
        )
        .await
        .expect("must stop at the deadline")
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Couldn't install \"koyuki launcher.zip\".\n\
             Step: downloading it\n\
             Cause: the download was still in progress after 15 minutes: the file is still empty and the browser \
             is still writing \"koyuki launcher.zip.part\"\n\
             Hint: Once the browser has finished the download, use \"I already downloaded it -- choose file\", or \
             add the file with Add."
        );
    }

    /// A paused download whose `.part` went stale: the placeholder is
    /// taken as final (empty) and handed to the install, which reports it.
    #[tokio::test]
    async fn a_paused_download_with_a_stale_part_is_handed_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mod.zip");
        let part = dir.path().join("mod.zip.part");
        touch(&path, b"").await;
        touch(&part, b"PK\x03\x04half").await;
        age(&part, Duration::from_secs(120));
        let found = tokio::time::timeout(POLL_INTERVAL * 6, wait_until_stable(&path, &AtomicBool::new(false), short_limits(60)))
            .await
            .expect("must not hang")
            .unwrap();
        assert_eq!(found.as_deref(), Some(path.as_path()));
    }

    #[tokio::test]
    async fn a_file_that_vanishes_mid_wait_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let err = wait_until_stable(&dir.path().join("gone.zip"), &AtomicBool::new(false), short_limits(60))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.starts_with("Couldn't install \"gone.zip\".\nStep: downloading it\nCause: the file disappeared"), "{msg}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_download_is_refused_in_the_same_format() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("real.zip"), b"PK\x03\x04").await;
        let link = dir.path().join("link.zip");
        tokio::fs::symlink(dir.path().join("real.zip"), &link).await.unwrap();
        let msg = wait_until_stable(&link, &AtomicBool::new(false), short_limits(60)).await.unwrap_err().to_string();
        assert!(msg.starts_with("Couldn't install \"link.zip\".\nStep: downloading it\nCause: the file in the Downloads folder is a symbolic link"), "{msg}");
    }

    #[tokio::test]
    async fn an_unreadable_downloads_folder_is_reported_in_the_same_format() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no such folder");
        let e = snapshot_names(&missing).await.unwrap_err();
        let msg = downloads_folder_error("https://example.com/mod", &missing, e).to_string();
        assert!(msg.starts_with("Couldn't install \"https://example.com/mod\".\nStep: downloading it\nCause: reading the Downloads folder ("), "{msg}");
        assert!(msg.ends_with("Hint: Check that the Downloads folder in Settings exists and DDMM can open it, then try again."), "{msg}");
    }

    /// The handoff used to take the empty placeholder as "finished" after
    /// two unchanged polls (0 bytes, 0 bytes) and fail to install it; a
    /// download that takes longer than that must be waited for.
    #[tokio::test]
    async fn wait_until_stable_waits_out_a_firefox_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koyuki launcher.zip");
        let part = dir.path().join("koyuki launcher.zip.part");
        touch(&path, b"").await;
        touch(&part, b"PK\x03\x04").await;

        let cancel = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (path, cancel) = (path.clone(), cancel.clone());
            tokio::spawn(async move { wait_until_stable(&path, &cancel, long_limits()).await })
        };

        // Long past the old "two unchanged polls".
        tokio::time::sleep(POLL_INTERVAL * 4).await;
        assert!(!waiter.is_finished(), "must not take the empty placeholder");

        // Firefox finishes: the .part replaces the placeholder.
        tokio::fs::write(&part, b"PK\x03\x04the whole archive").await.unwrap();
        tokio::fs::rename(&part, &path).await.unwrap();

        let found = tokio::time::timeout(POLL_INTERVAL * 6, waiter).await.unwrap().unwrap().unwrap();
        assert_eq!(found.as_deref(), Some(path.as_path()));
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"PK\x03\x04the whole archive");
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
        let result = wait_until_stable(&path, &cancel, long_limits()).await.unwrap();
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
