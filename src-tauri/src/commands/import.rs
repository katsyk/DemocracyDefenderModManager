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
    *state.import_scan.lock().await = Some(scan.clone());
    let free_bytes = crate::data_move::available_space(&state.base_path.join(MODS_DIRECTORY));
    Ok(ScanSummary { scan, free_bytes, cancelled })
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

/// Import the items (by id) of the last scan. Refused while the data
/// folder is being moved; checks free space first; one at a time, with
/// progress events and cancel; never stops for one failing mod.
#[tauri::command]
pub async fn run_import(app: AppHandle, state: State<'_, AppState>, ids: Vec<usize>) -> TAResult<ImportReport> {
    let _data_op = state.data_op().into_ta_result()?;
    let (cancel, _slot) = begin(&state).into_ta_result()?;

    let items: Vec<_> = {
        let scan = state.import_scan.lock().await;
        let Some(scan) = scan.as_ref() else {
            return anyhow::anyhow!("Nothing was scanned yet; scan a folder first.").into_ta_result();
        };
        scan.items.iter().filter(|i| ids.contains(&i.id) && !i.status.blocked()).cloned().collect()
    };
    if items.is_empty() {
        return Ok(ImportReport::default());
    }

    let needed: u64 = items.iter().map(|i| i.size).sum();
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
        "Import finished: {} imported, {} failed{}",
        report.imported.len(),
        report.failed.len(),
        if report.cancelled { ", cancelled" } else { "" }
    );
    *state.import_scan.lock().await = None;
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
