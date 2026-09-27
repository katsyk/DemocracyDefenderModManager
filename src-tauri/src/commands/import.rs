//! "Import mods": the commands behind the import wizard. The work itself is
//! in [`crate::mod_import`]; this only wires it to the app (locks, events,
//! cancel, the last scan).

use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

use anyhow_tauri::{IntoTAResult, TAResult};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{
    commands::{mods::{ensure_mods_loaded, MODS_DIRECTORY}, settings::do_load_settings},
    mod_import::{self, DetectedSource, ImportReport, InstalledIndex, KnownDirs, ScanResult},
    AppState,
};

const SCAN_PROGRESS_EVENT: &str = "import://scan-progress";
const PROGRESS_EVENT: &str = "import://progress";
/// Progress events are throttled to this rate.
const EMIT_EVERY: Duration = Duration::from_millis(100);

/// Clears the running-import marker when the command ends, however it
/// ends.
struct CancelSlot<'a>(&'a AppState);

impl Drop for CancelSlot<'_> {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.0.import_cancel.lock() {
            *slot = None;
        }
    }
}

/// Register a new cancellable scan/import; only one runs at a time.
fn begin(state: &AppState) -> anyhow::Result<(Arc<AtomicBool>, CancelSlot<'_>)> {
    let mut slot = state.import_cancel.lock().map_err(|_| anyhow::anyhow!("import state poisoned"))?;
    if slot.is_some() {
        anyhow::bail!("An import is already running. Wait for it to finish, or cancel it.");
    }
    let flag = Arc::new(AtomicBool::new(false));
    *slot = Some(flag.clone());
    drop(slot);
    Ok((flag, CancelSlot(state)))
}

async fn installed_index(state: &AppState) -> TAResult<InstalledIndex> {
    let mods = {
        let mut guard = state.mods.lock().await;
        ensure_mods_loaded(&mut guard, &state.base_path).await?.clone()
    };
    Ok(InstalledIndex::build(&mods).await)
}

/// The data folders of other mod managers (and the Downloads folder) that
/// exist on this machine and hold something to import.
#[tauri::command]
pub async fn detect_import_sources(state: State<'_, AppState>) -> TAResult<Vec<DetectedSource>> {
    let settings_downloads = do_load_settings(&state.base_path)
        .await
        .ok()
        .map(|s| s.downloads_path().to_path_buf())
        .filter(|p| !p.as_os_str().is_empty());
    let base = state.base_path.clone();
    let found = tokio::task::spawn_blocking(move || {
        mod_import::detect_sources(&KnownDirs::for_this_machine(settings_downloads), &base)
    })
    .await
    .into_ta_result()?;
    log::info!("Import: found {} possible source(s): {:?}", found.len(), found.iter().map(|s| &s.path).collect::<Vec<_>>());
    Ok(found)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScanSummary {
    #[serde(flatten)]
    pub scan: ScanResult,
    /// Free space on the drive with DDMM's data folder, if known.
    pub free_bytes: Option<u64>,
    pub cancelled: bool,
    /// Which scan this is. [`run_import`] takes it back, so a wizard can
    /// only import from its own scan: item ids are per scan, and a newer
    /// scan replaces the stored one.
    pub scan_token: u64,
}

/// A new, never-repeating [`ScanSummary::scan_token`].
fn next_scan_token() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

async fn finish_scan(
    app: &AppHandle,
    state: &AppState,
    run: impl FnOnce(&InstalledIndex, &AtomicBool, &mut dyn FnMut(mod_import::ScanProgress)) -> anyhow::Result<ScanResult>
        + Send
        + 'static,
) -> TAResult<ScanSummary> {
    let (cancel, _slot) = begin(state).into_ta_result()?;
    let index = installed_index(state).await?;
    let app_for_progress = app.clone();
    let cancel_for_scan = cancel.clone();
    let scan = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now() - EMIT_EVERY;
        let mut emit = |p: mod_import::ScanProgress| {
            if p.done == p.total || last.elapsed() >= EMIT_EVERY {
                last = Instant::now();
                let _ = app_for_progress.emit(SCAN_PROGRESS_EVENT, p);
            }
        };
        run(&index, &cancel_for_scan, &mut emit)
    })
    .await
    .into_ta_result()?
    .into_ta_result()?;
    let cancelled = cancel.load(std::sync::atomic::Ordering::SeqCst);
    log::info!(
        "Import scan of {:?}: {} item(s){}",
        scan.root,
        scan.items.len(),
        if cancelled { " (cancelled)" } else { "" }
    );
    let scan_token = next_scan_token();
    *state.import_scan.lock().await = Some((scan_token, scan.clone()));
    let free_bytes = crate::data_move::available_space(&state.base_path.join(MODS_DIRECTORY));
    Ok(ScanSummary { scan, free_bytes, cancelled, scan_token })
}

/// Scan a folder -- another manager's mods, or a folder of archives -- for
/// mods to import. Nothing is changed anywhere.
#[tauri::command]
pub async fn scan_import_folder(app: AppHandle, state: State<'_, AppState>, folder: PathBuf) -> TAResult<ScanSummary> {
    log::info!("Import: scanning {folder:?}...");
    mod_import::ensure_source_allowed(&folder, &state.base_path).into_ta_result()?;
    finish_scan(&app, &state, move |index, cancel, progress| {
        mod_import::scan_folder(&folder, index, cancel, progress)
    })
    .await
}

/// Same as [`scan_import_folder`] for an explicit list of archives/folders
/// (many files picked with Add, or dropped on the window).
#[tauri::command]
pub async fn scan_import_paths(app: AppHandle, state: State<'_, AppState>, paths: Vec<PathBuf>) -> TAResult<ScanSummary> {
    log::info!("Import: scanning {} picked path(s)...", paths.len());
    finish_scan(&app, &state, move |index, cancel, progress| Ok(mod_import::scan_paths(&paths, index, cancel, progress)))
        .await
}

/// The chosen items (by id) of the scan `scan_token` names, or why they
/// can't be had: nothing scanned, or a newer scan replaced that one (its
/// ids would pick other mods).
async fn chosen_items(state: &AppState, scan_token: u64, ids: &[usize]) -> anyhow::Result<Vec<mod_import::ScanItem>> {
    let scan = state.import_scan.lock().await;
    let Some((token, scan)) = scan.as_ref() else {
        anyhow::bail!("Nothing was scanned yet, or this list was already imported. Scan the folder again.");
    };
    if *token != scan_token {
        anyhow::bail!(
            "This list is out of date: another scan was started since. Go back and scan again to import from it."
        );
    }
    Ok(scan.items.iter().filter(|i| ids.contains(&i.id) && !i.status.blocked()).cloned().collect())
}

/// Import the items (by id) of the scan `scan_token` names. Refused while
/// the data folder is being moved, or when that scan isn't the last one;
/// checks free space first; one at a time, with progress events and
/// cancel; never stops for one failing mod.
#[tauri::command]
pub async fn run_import(
    app: AppHandle,
    state: State<'_, AppState>,
    scan_token: u64,
    ids: Vec<usize>,
) -> TAResult<ImportReport> {
    let _data_op = state.data_op().into_ta_result()?;
    let (cancel, _slot) = begin(&state).into_ta_result()?;

    let items = chosen_items(&state, scan_token, &ids).await.into_ta_result()?;
    if items.is_empty() {
        return Ok(ImportReport::default());
    }

    let needed: u64 = items.iter().filter(|i| i.link_to.is_none()).map(|i| i.size).sum();
    mod_import::check_free_space(needed, crate::data_move::available_space(&state.base_path.join(MODS_DIRECTORY)))
        .into_ta_result()?;

    // Make sure the mod list is loaded before installing into it.
    {
        let mut guard = state.mods.lock().await;
        ensure_mods_loaded(&mut guard, &state.base_path).await?;
    }

    log::info!("Import: importing {} mod(s), about {} unpacked...", items.len(), crate::data_move::human_bytes(needed));
    let mut last = Instant::now() - EMIT_EVERY;
    let report = mod_import::run_import(&state, items, &cancel, |p| {
        if p.current.is_none() || last.elapsed() >= EMIT_EVERY {
            last = Instant::now();
            let _ = app.emit(PROGRESS_EVENT, p);
        }
    })
    .await;
    log::info!(
        "Import finished: {} imported, {} linked, {} failed{}",
        report.imported.len(),
        report.linked.len(),
        report.failed.len(),
        if report.cancelled { ", cancelled" } else { "" }
    );
    {
        // Only this wizard's scan is used up; a newer one stays.
        let mut stored = state.import_scan.lock().await;
        if stored.as_ref().is_some_and(|(token, _)| *token == scan_token) {
            *stored = None;
        }
    }
    Ok(report)
}

/// Stop the running scan or import after the current mod (which is
/// removed again if it was being imported).
#[tauri::command]
pub async fn cancel_import(state: State<'_, AppState>) -> TAResult<()> {
    if let Ok(slot) = state.import_cancel.lock() {
        if let Some(flag) = slot.as_ref() {
            log::info!("Import: cancel requested.");
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_import::{ItemKind, ItemStatus, ScanItem};

    fn item(id: usize, name: &str) -> ScanItem {
        ScanItem {
            id,
            kind: ItemKind::Archive,
            path: PathBuf::from(format!("{name}.zip")),
            name: name.into(),
            size: 1,
            file_size: 1,
            guid: None,
            nexus: None,
            status: ItemStatus::New,
            profile: None,
            carried_sidecar: None,
            sha256: None,
            local_guid: false,
            manifest_override: None,
            description: None,
            link_to: None,
        }
    }

    fn scan(items: Vec<ScanItem>) -> ScanResult {
        ScanResult { root: None, items, profile_name: None, truncated: false }
    }

    /// Two wizards (a second one opened by a drop): the first must not
    /// import the second scan's items, which have the same ids.
    #[tokio::test]
    async fn an_import_only_takes_items_from_its_own_scan() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let first = next_scan_token();
        *state.import_scan.lock().await = Some((first, scan(vec![item(0, "A0"), item(1, "A1")])));
        let second = next_scan_token();
        assert_ne!(first, second);
        *state.import_scan.lock().await = Some((second, scan(vec![item(0, "B0"), item(1, "B1")])));

        let err = chosen_items(&state, first, &[0, 1]).await.unwrap_err();
        assert!(format!("{err}").contains("out of date"), "{err}");
        let items = chosen_items(&state, second, &[1]).await.unwrap();
        assert_eq!(items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["B1"]);

        *state.import_scan.lock().await = None;
        assert!(chosen_items(&state, second, &[1]).await.is_err());
    }
}
