//! Settings' data folder controls ("Change", "Reset to default") and the
//! recovery screen shown when the chosen data folder is missing. The file
//! work itself is in `crate::data_move`; where the choice is stored is in
//! `crate::data_dir`. See `docs/using/data-folder.md`.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context;
use anyhow_tauri::{IntoTAResult, TAResult};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    commands::settings::do_load_settings,
    data_dir::{self, BaseDirKind, DataDirProblem},
    data_move::{self, MovePlan, MoveProgress, PlanInput},
    AppState,
};

pub const PROGRESS_EVENT: &str = "data-folder://progress";
/// How long a move waits for running operations (an install, an update
/// check, ...) to finish before giving up.
const WAIT_FOR_OPERATIONS: Duration = Duration::from_secs(15);
/// Gives the frontend time to show "Restarting..." before the app restarts.
const RESTART_DELAY: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DataFolderInfo {
    pub path: String,
    pub default_path: String,
    /// The data folder was chosen in Settings (a pointer file exists).
    pub is_custom: bool,
    pub portable: bool,
    /// Where the choice is stored.
    pub pointer_file: String,
    /// `"missing"` or `"bad_pointer"` in recovery mode.
    pub problem: Option<&'static str>,
    pub problem_detail: Option<String>,
}

#[tauri::command]
pub fn get_data_folder_info(state: State<'_, AppState>) -> DataFolderInfo {
    let d = &state.data_dir;
    let (problem, problem_detail) = match &d.problem {
        None => (None, None),
        Some(DataDirProblem::Missing) => (Some("missing"), None),
        Some(DataDirProblem::BadPointer(m)) => (Some("bad_pointer"), Some(m.clone())),
    };
    DataFolderInfo {
        path: d.path.to_string_lossy().into_owned(),
        default_path: d.default_path.to_string_lossy().into_owned(),
        is_custom: d.pointed,
        portable: d.kind == BaseDirKind::Portable,
        pointer_file: d.pointer_file.to_string_lossy().into_owned(),
        problem,
        problem_detail,
    }
}

async fn make_plan(state: &AppState, destination: Option<String>, reset: bool) -> anyhow::Result<MovePlan> {
    if state.data_dir.problem.is_some() {
        anyhow::bail!("The data folder isn't available; use the options on the recovery screen instead.");
    }
    let game_path = do_load_settings(&state.base_path)
        .await
        .ok()
        .map(|s| s.game_path().to_path_buf())
        .filter(|p| !p.as_os_str().is_empty());
    let current = state.base_path.clone();
    let default_path = state.data_dir.default_path.clone();
    let picked = match destination {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ if reset => default_path.clone(),
        _ => anyhow::bail!("Pick a folder first."),
    };
    tokio::task::spawn_blocking(move || {
        data_move::plan(
            &PlanInput {
                current: &current,
                picked: &picked,
                default_path: &default_path,
                game_path: game_path.as_deref(),
                reset,
            },
            &data_move::available_space,
        )
    })
    .await?
}

/// Check a folder the user picked (or, with `reset`, the default location)
/// and say what moving there would do. Changes nothing.
#[tauri::command]
pub async fn plan_data_folder_move(
    state: State<'_, AppState>,
    destination: Option<String>,
    reset: bool,
) -> TAResult<MovePlan> {
    let plan = make_plan(&state, destination, reset).await;
    if let Err(e) = &plan {
        log::info!("Data folder move refused: {e:#}");
    }
    plan.into_ta_result()
}

/// Wait for running operations to finish, then keep new ones from starting.
async fn lock_data_ops(state: &AppState) -> anyhow::Result<tokio::sync::OwnedRwLockWriteGuard<()>> {
    if state.handoff_cancel.lock().await.is_some() {
        anyhow::bail!("A browser download is still being watched for. Cancel or finish it first.");
    }
    tokio::time::timeout(WAIT_FOR_OPERATIONS, state.data_ops.clone().write_owned())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "DDMM is still busy (an install, update or check is running, or waiting for your \
                 answer). Finish it and try again."
            )
        })
}

/// Store the new location: write the pointer file, or remove it when going
/// back to the default. A portable copy going back to its own folder gets a
/// `portable.txt` so it's still recognized as portable without the pointer.
fn commit_location(state_dir: &data_dir::DataDirDecision, target: &Path, is_reset: bool) -> anyhow::Result<()> {
    if is_reset {
        if state_dir.kind == BaseDirKind::Portable {
            let marker = state_dir.default_path.join(data_dir::PORTABLE_MARKER_FILENAME);
            if !marker.exists() {
                std::fs::write(&marker, b"")
                    .with_context(|| format!("couldn't create {}", marker.display()))?;
            }
        }
        data_dir::remove_pointer(&state_dir.pointer_file)
    } else {
        data_dir::write_pointer(&state_dir.pointer_file, target).map_err(|e| {
            if state_dir.kind == BaseDirKind::Portable {
                e.context(format!(
                    "DDMM keeps the data folder location next to its program file, in {}",
                    state_dir.pointer_file.display()
                ))
            } else {
                e
            }
        })
    }
}

/// Exit through the normal path (bridge shutdown, single-instance lock
/// released) and start the app again **without this process's
/// arguments** (only install links that were never shown): not
/// `AppHandle::request_restart`, which replays this process's arguments
/// (a `ddmm://` link would prompt again). See `app_lifecycle`.
fn restart_soon(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RESTART_DELAY).await;
        log::info!("Restarting to use the new data folder.");
        app.state::<AppState>().relaunch_on_exit.store(true, std::sync::atomic::Ordering::SeqCst);
        app.exit(0);
    });
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MoveResult {
    pub target: String,
    /// Old items that couldn't be deleted yet; retried at the next start.
    pub leftovers: Vec<String>,
}

/// Move DDMM's data to `destination` (or back to the default location with
/// `reset`), then restart. On failure nothing is changed.
#[tauri::command]
pub async fn move_data_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    destination: Option<String>,
    reset: bool,
) -> TAResult<MoveResult> {
    let guard = lock_data_ops(&state).await.into_ta_result()?;
    let plan = make_plan(&state, destination, reset).await.into_ta_result()?;
    if plan.existing_data {
        return anyhow::anyhow!(
            "{} already holds DDMM data. Use it as is, or pick another folder.",
            plan.target.display()
        )
        .into_ta_result();
    }
    log::info!(
        "Moving the data folder from {:?} to {:?} ({} files, {})",
        plan.source,
        plan.target,
        plan.total_files,
        data_move::human_bytes(plan.total_bytes)
    );

    state.data_move_running.store(true, std::sync::atomic::Ordering::SeqCst);
    let decision = state.data_dir.clone();
    let progress_app = app.clone();
    let blocking_plan = plan.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut last_emit: Option<Instant> = None;
        let mut progress = |p: &MoveProgress| {
            let finished = p.done_files == p.total_files || p.phase != "copying";
            if finished || last_emit.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                last_emit = Some(Instant::now());
                let _ = progress_app.emit(PROGRESS_EVENT, p);
            }
        };
        let mut commit = |target: &Path| commit_location(&decision, target, blocking_plan.is_reset);
        data_move::execute(&blocking_plan, &data_move::REAL_OPS, &mut commit, &mut progress)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|r| r);
    state.data_move_running.store(false, std::sync::atomic::Ordering::SeqCst);

    if let Err(e) = result {
        log::error!("Moving the data folder failed; nothing was changed: {e:#}");
        return Err(e).into_ta_result();
    }

    log::info!("Data folder moved to {:?}; removing the old copy.", plan.target);
    let default_path = state.data_dir.default_path.clone();
    let cleanup_plan = plan.clone();
    let leftovers = tokio::task::spawn_blocking(move || data_move::cleanup_old(&cleanup_plan, &default_path))
        .await
        .unwrap_or_default();

    // Nothing may write to the old folder any more; the restart picks up
    // the new one everywhere (asset scope, bridge.json, logs, browser
    // registration).
    state.freeze_data_ops(Some(guard));
    restart_soon(app);

    Ok(MoveResult {
        target: plan.target.to_string_lossy().into_owned(),
        leftovers: leftovers.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    })
}

/// Switch to a folder that already holds DDMM data, without copying or
/// deleting anything, then restart.
#[tauri::command]
pub async fn adopt_data_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    destination: Option<String>,
    reset: bool,
) -> TAResult<String> {
    let guard = lock_data_ops(&state).await.into_ta_result()?;
    let plan = make_plan(&state, destination, reset).await.into_ta_result()?;
    if !plan.existing_data {
        return anyhow::anyhow!("{} doesn't hold DDMM data.", plan.target.display()).into_ta_result();
    }
    commit_location(&state.data_dir, &plan.target, plan.is_reset).into_ta_result()?;
    log::info!("Now using the existing DDMM data in {:?} (the old folder {:?} was left as it was).", plan.target, plan.source);
    state.freeze_data_ops(Some(guard));
    restart_soon(app);
    Ok(plan.target.to_string_lossy().into_owned())
}

/// Recovery screen: check again whether the chosen folder is back (a USB
/// drive plugged in again) and restart if it is.
#[tauri::command]
pub async fn retry_data_folder(app: AppHandle) -> TAResult<()> {
    let decision = tokio::task::spawn_blocking(data_dir::decide_data_dir_for_this_machine)
        .await
        .into_ta_result()?;
    match decision.problem {
        None => {
            restart_soon(app);
            Ok(())
        }
        Some(DataDirProblem::Missing) => {
            anyhow::anyhow!("{} still isn't there.", decision.path.display()).into_ta_result()
        }
        Some(DataDirProblem::BadPointer(m)) => anyhow::anyhow!(m).into_ta_result(),
    }
}

/// Recovery screen: point DDMM at the folder its data is in now (e.g. the
/// drive letter changed), then restart.
#[tauri::command]
pub async fn locate_data_folder(app: AppHandle, state: State<'_, AppState>, folder: String) -> TAResult<()> {
    let folder = PathBuf::from(folder.trim());
    if !folder.is_dir() {
        return anyhow::anyhow!("{} doesn't exist or isn't a folder.", folder.display()).into_ta_result();
    }
    if folder.join(data_move::INCOMPLETE_MARKER).exists() {
        return anyhow::anyhow!(
            "{} holds an unfinished data folder move, not complete DDMM data.",
            folder.display()
        )
        .into_ta_result();
    }
    let chosen = if data_move::is_ddmm_data_dir(&folder) {
        folder
    } else if data_move::is_ddmm_data_dir(&folder.join(data_move::SUBFOLDER_NAME)) {
        folder.join(data_move::SUBFOLDER_NAME)
    } else {
        return anyhow::anyhow!(
            "{} doesn't look like a DDMM data folder (no settings.json or profiles.json from DDMM in it).",
            folder.display()
        )
        .into_ta_result();
    };
    data_dir::write_pointer(&state.data_dir.pointer_file, &chosen).into_ta_result()?;
    log::info!("Data folder located at {chosen:?}; restarting.");
    restart_soon(app);
    Ok(())
}

/// Recovery screen: forget the chosen folder and go back to the default
/// location (which may be empty), then restart. The missing folder itself
/// is never touched.
#[tauri::command]
pub async fn reset_data_folder_location(app: AppHandle, state: State<'_, AppState>) -> TAResult<()> {
    let d = &state.data_dir;
    if d.kind == BaseDirKind::Portable {
        let marker = d.default_path.join(data_dir::PORTABLE_MARKER_FILENAME);
        if !marker.exists() {
            if let Err(e) = std::fs::write(&marker, b"") {
                log::warn!("Couldn't create {marker:?}: {e}");
            }
        }
    }
    data_dir::remove_pointer(&d.pointer_file).into_ta_result()?;
    log::info!("Data folder location reset to the default ({:?}); restarting.", d.default_path);
    restart_soon(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(root: &Path, kind: BaseDirKind) -> data_dir::DataDirDecision {
        let default_path = root.join("default");
        std::fs::create_dir_all(&default_path).unwrap();
        data_dir::DataDirDecision {
            path: default_path.clone(),
            kind,
            reason: String::new(),
            pointer_file: default_path.join(data_dir::POINTER_FILENAME),
            default_path,
            pointed: false,
            problem: None,
        }
    }

    #[test]
    fn adopting_existing_data_only_writes_the_pointer() {
        let root = tempfile::tempdir().unwrap();
        let d = decision(root.path(), BaseDirKind::AppData);
        std::fs::create_dir_all(d.path.join("mods/ModA")).unwrap();
        std::fs::write(d.path.join("profiles.json"), br#"{"Profiles":[],"Active":0}"#).unwrap();
        let usb = root.path().join("usb");
        std::fs::create_dir(&usb).unwrap();
        std::fs::write(usb.join("settings.json"), br#"{"Version":"V1","GamePath":"","SkipList":[]}"#).unwrap();

        let plan = data_move::plan(
            &PlanInput { current: &d.path, picked: &usb, default_path: &d.default_path, game_path: None, reset: false },
            &|_| None,
        )
        .unwrap();
        assert!(plan.existing_data);
        commit_location(&d, &plan.target, plan.is_reset).unwrap();

        let decided = data_dir::decide_data_dir(&root.path().join("exe"), &d.default_path, &d.default_path);
        assert_eq!(decided.path, usb);
        // Nothing was copied or deleted either way.
        assert!(d.path.join("mods/ModA").is_dir());
        assert!(!usb.join("mods").exists());
    }

    #[tokio::test]
    async fn data_operations_are_refused_during_and_after_a_move() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(root.path().to_path_buf());
        {
            let _op = state.data_op().unwrap();
            // A move waits for running operations...
            let waited = tokio::time::timeout(Duration::from_millis(50), state.data_ops.clone().write_owned()).await;
            assert!(waited.is_err());
        }
        let guard = lock_data_ops(&state).await.unwrap();
        // ...and while it runs, new ones fail at once instead of waiting.
        assert!(state.data_op().is_err());
        assert!(state.data_ops_paused());
        state.freeze_data_ops(Some(guard));
        assert!(format!("{:#}", state.data_op().unwrap_err()).contains("being moved"));
    }

    #[test]
    fn resetting_a_portable_copy_removes_the_pointer_and_keeps_it_portable() {
        let root = tempfile::tempdir().unwrap();
        let d = decision(root.path(), BaseDirKind::Portable);
        let custom = root.path().join("custom");
        std::fs::create_dir(&custom).unwrap();
        commit_location(&d, &custom, false).unwrap();
        assert_eq!(data_dir::read_pointer(&d.pointer_file).unwrap(), Some(custom.clone()));

        commit_location(&d, &d.default_path, true).unwrap();
        assert!(!d.pointer_file.exists());
        assert!(d.default_path.join(data_dir::PORTABLE_MARKER_FILENAME).is_file());
        let decided =
            data_dir::decide_data_dir(&d.default_path, &root.path().join("appdata"), &root.path().join("config"));
        assert_eq!(decided.kind, BaseDirKind::Portable);
        assert_eq!(decided.path, d.default_path);
    }
}
