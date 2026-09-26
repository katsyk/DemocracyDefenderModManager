pub mod commands;
pub mod models;
pub mod archive;
pub mod utils;
pub mod fs_util;
pub mod game_path;
pub mod sources;
pub mod download;
pub mod data_dir;
pub mod steam;
pub mod bridge;
pub mod deep_link;
pub mod auto_import;
pub mod data_move;
pub mod app_lifecycle;
pub mod providers;
pub mod secrets;
pub mod nexus_oauth;
pub mod mod_import;
pub mod install_error;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use log::LevelFilter;
use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

/// Log rotation: 5 MB per file, keeping the 3 most recent rotated files
/// alongside the current one.
const LOG_MAX_FILE_BYTES: u128 = 5 * 1024 * 1024;
const LOG_KEEP_FILES: usize = 3;
use tokio::sync::Mutex;

use crate::{bridge::BridgePending, models::Mod};

pub struct AppState {
    base_path: PathBuf,
    /// How `base_path` was chosen (pointer file, default, problem); see
    /// `data_dir::decide_data_dir`.
    data_dir: data_dir::DataDirDecision,
    /// Read-locked by every operation that writes to the data folder,
    /// write-locked while it's being moved. See [`AppState::data_op`].
    data_ops: Arc<tokio::sync::RwLock<()>>,
    /// Holds the write lock for good once the data folder has been moved
    /// (until the restart), and in recovery mode.
    data_ops_frozen: std::sync::Mutex<Option<tokio::sync::OwnedRwLockWriteGuard<()>>>,
    mods: Mutex<Option<Vec<Mod>>>,
    /// Cancellation flag for the single in-flight browser handoff, if any.
    /// `commands::handoff` is the only thing that touches this.
    handoff_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Consent/install-completion round trips the bridge server is
    /// waiting on the frontend to resolve. `bridge::server` only.
    bridge_pending: Mutex<BridgePending>,
    /// How many bridge installs are currently queued (including the one
    /// running); see `bridge::protocol::MAX_QUEUE_DEPTH`.
    bridge_queue_depth: AtomicUsize,
    /// Serializes actual bridge installs to one at a time.
    bridge_install_lock: Mutex<()>,
    /// `true` while the Mods page is mounted with its bridge listeners
    /// registered and profiles loaded -- i.e. while a consent prompt or an
    /// `afterInstall` step emitted now would actually be handled. Set by
    /// `commands::bridge::set_bridge_frontend_ready`; awaited by
    /// `bridge::server` before emitting either event (on a cold start the
    /// extension's install arrives before the webview has loaded).
    bridge_frontend_ready: tokio::sync::watch::Sender<bool>,
    /// Set by `commands::ack_close_requested` -- the frontend's close
    /// handler calls it as the very first thing it does, so this being
    /// `true` proves the frontend is alive and actually handling the close
    /// (as opposed to never having loaded, having crashed, or the IPC
    /// bridge itself being broken). Reset to `false` each time a new close
    /// is requested; read by the close watchdog in `run()`.
    close_ack: AtomicBool,
    /// `true` while `commands::data_folder::move_data_folder` is copying;
    /// closing the window is refused meanwhile.
    data_move_running: AtomicBool,
    /// Set once a data folder change is committed: on exit, start the app
    /// again (with no arguments; see `app_lifecycle::relaunch_command`).
    relaunch_on_exit: AtomicBool,
    /// Serializes update checks (manual, startup, scheduled).
    update_check_lock: Mutex<()>,
    /// The latest update check's results this session -- see
    /// `commands::updates`.
    last_update_report: Mutex<Option<commands::updates::UpdateCheckReport>>,
    /// When the latest update check ran (for the opt-in re-check interval).
    last_update_check: Mutex<Option<tokio::time::Instant>>,
    /// When the browser extension last talked to this session (any bridge
    /// request) -- decides whether a browser update can finish with the
    /// extension's "Update with DDMM" button.
    bridge_last_seen: Mutex<Option<tokio::time::Instant>>,
    /// Cancels the in-flight "Sign in to Nexus Mods", if any
    /// (`commands::nexus` only).
    nexus_sign_in_cancel: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Cancel flag of the running import scan or import, if any
    /// (`commands::import` only; one at a time).
    import_cancel: std::sync::Mutex<Option<Arc<AtomicBool>>>,
    /// The latest import scan, which `run_import` imports from by item id.
    import_scan: Mutex<Option<mod_import::ScanResult>>,
}

/// Shown when something that writes to the data folder is attempted while
/// it's being moved or isn't available.
pub const DATA_FOLDER_BUSY: &str =
    "DDMM's data folder is being moved or isn't available right now. Wait for DDMM to restart, then try again.";

impl AppState {
    pub fn new(base_path: PathBuf) -> Self {
        let decision = data_dir::DataDirDecision {
            path: base_path.clone(),
            kind: data_dir::BaseDirKind::AppData,
            reason: String::new(),
            default_path: base_path.clone(),
            pointer_file: base_path.join(data_dir::POINTER_FILENAME),
            pointed: false,
            problem: None,
        };
        Self::with_decision(decision)
    }

    pub fn with_decision(decision: data_dir::DataDirDecision) -> Self {
        Self {
            base_path: decision.path.clone(),
            data_dir: decision,
            data_ops: Arc::new(tokio::sync::RwLock::new(())),
            data_ops_frozen: std::sync::Mutex::new(None),
            mods: Mutex::default(),
            handoff_cancel: Mutex::default(),
            bridge_pending: Mutex::default(),
            bridge_queue_depth: AtomicUsize::new(0),
            bridge_install_lock: Mutex::new(()),
            bridge_frontend_ready: tokio::sync::watch::Sender::new(false),
            close_ack: AtomicBool::new(false),
            data_move_running: AtomicBool::new(false),
            relaunch_on_exit: AtomicBool::new(false),
            update_check_lock: Mutex::new(()),
            last_update_report: Mutex::default(),
            last_update_check: Mutex::default(),
            bridge_last_seen: Mutex::default(),
            nexus_sign_in_cancel: Mutex::default(),
            import_cancel: std::sync::Mutex::default(),
            import_scan: Mutex::default(),
        }
    }

    /// Permission to write to the data folder, held for the length of one
    /// operation (installing, deleting, deploying, saving settings, an
    /// update check, ...). Fails at once, rather than waiting, while the
    /// data folder is being moved, after a move until the restart, and in
    /// the missing-folder recovery screen.
    pub fn data_op(&self) -> anyhow::Result<tokio::sync::OwnedRwLockReadGuard<()>> {
        self.data_ops.clone().try_read_owned().map_err(|_| anyhow::anyhow!(DATA_FOLDER_BUSY))
    }

    /// Whether [`AppState::data_op`] would fail right now (background
    /// tasks use this to skip a tick).
    pub fn data_ops_paused(&self) -> bool {
        self.data_ops.try_read().is_err()
    }

    /// Block every data operation until the app exits (after a successful
    /// move, and in recovery mode). `guard` is the write lock the caller
    /// already holds, if any.
    pub fn freeze_data_ops(&self, guard: Option<tokio::sync::OwnedRwLockWriteGuard<()>>) {
        let guard = guard.or_else(|| self.data_ops.clone().try_write_owned().ok());
        if let Ok(mut slot) = self.data_ops_frozen.lock() {
            *slot = guard;
        }
    }
}

/// Compute the base data directory the same way the desktop app does,
/// without touching Tauri at all -- used both by `run()` before the
/// builder is constructed and by host mode, which never constructs one.
fn resolve_base_path() -> PathBuf {
    data_dir::decide_data_dir_for_this_machine().path
}

/// Run as the browser's native-messaging host: relay only, no window, no
/// Tauri runtime at all. Must be checked before anything Tauri-related is
/// touched, since a host-mode invocation has no display to attach to.
///
/// The host only ever reads `bridge.json`; it never creates the data
/// folder (a chosen folder that's missing must stay missing, see
/// `data_dir::DataDirProblem`).
fn run_host_mode(origin: bridge::host::HostOrigin) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start host-mode runtime: {e}");
            return;
        }
    };
    runtime.block_on(bridge::host::run(origin, resolve_base_path));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(origin) = bridge::host::detect_host_mode(&args) {
        run_host_mode(origin);
        return;
    }

    let decision = data_dir::decide_data_dir_for_this_machine();
    // Recovery mode: the chosen data folder is missing (or its pointer file
    // is unreadable). Never create it -- that would turn an unplugged drive
    // into an empty mod list -- and start nothing that touches it; the
    // frontend shows the recovery screen instead (see
    // `commands::data_folder`).
    let recovery = decision.problem.is_some();
    if !recovery {
        if let Err(e) = std::fs::create_dir_all(&decision.path) {
            eprintln!("warning: failed to create data directory {:?}: {}", decision.path, e);
        }
    }

    let base_path = decision.path.clone();
    let asset_base_path = base_path.clone();
    let log_dir = if recovery {
        std::env::temp_dir().join(format!("{}-logs", data_dir::APP_IDENTIFIER))
    } else {
        base_path.join("logs")
    };
    let _ = std::fs::create_dir_all(&log_dir);

    // The base-directory decision itself is logged again once the log
    // plugin (which needs this same `log_dir`) is up, so it actually lands
    // in the log file and not just stdout.
    let startup_message = format!(
        "Using {} directory: {:?} ({}){}",
        match decision.kind {
            data_dir::BaseDirKind::Portable => "portable data",
            data_dir::BaseDirKind::AppData => "app data",
        },
        base_path,
        decision.reason,
        match &decision.problem {
            None => String::new(),
            Some(data_dir::DataDirProblem::Missing) => {
                " -- THE FOLDER IS MISSING; showing the recovery screen".to_string()
            }
            Some(data_dir::DataDirProblem::BadPointer(m)) => format!(" -- {m}; showing the recovery screen"),
        }
    );
    let app_state = AppState::with_decision(decision.clone());
    if recovery {
        app_state.freeze_data_ops(None);
    }

    tauri::Builder::default()
        // Must be registered first (see the plugin's own docs): it needs
        // to intercept a second launch -- including one carrying a
        // `ddmm://` deep link on Windows/Linux, where the OS starts a new
        // process rather than emitting an event -- before anything else
        // runs.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            bridge::server::focus_main_window(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_prevent_default::debug())
        .plugin(
            tauri_plugin_log::Builder::new()
                // The plugin's default is a single 40 KB file, i.e. only a
                // few minutes of history; keep enough for a bug report.
                .max_file_size(LOG_MAX_FILE_BYTES)
                .rotation_strategy(RotationStrategy::KeepSome(LOG_KEEP_FILES))
                .level(if cfg!(debug_assertions) {
                    LevelFilter::Debug
                } else {
                    LevelFilter::Info
                })
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::Webview),
                    Target::new(TargetKind::Folder {
                        path: log_dir,
                        file_name: None
                    })
                ])
                .build()
        )
        .setup(move |app| {
            log::info!("{}", startup_message);

            if recovery {
                // Nothing below may touch the (missing) data folder: no
                // asset scope, bridge, browser registration, deep links,
                // auto-import or update checks. The recovery screen offers
                // Retry / Locate / Reset, each of which restarts the app.
                return Ok(());
            }

            // Old data a previous data folder move couldn't delete yet
            // (e.g. a log file Windows still had open). A little later, so
            // the previous process has certainly exited.
            let cleanup_base = asset_base_path.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let _ = tokio::task::spawn_blocking(move || data_move::run_pending_cleanup(&cleanup_base)).await;
            });

            // Mod icons and option images are shown with the asset protocol;
            // allow exactly the mods folder under the data folder, nothing
            // else (tauri.conf.json grants no static scope). After a data
            // folder move the app restarts, so this always follows the
            // current folder.
            let _ = std::fs::create_dir_all(asset_base_path.join(commands::mods::MODS_DIRECTORY));
            for dir in data_dir::asset_scope_dirs(&asset_base_path) {
                if let Err(e) = app.asset_protocol_scope().allow_directory(&dir, true) {
                    log::warn!("Couldn't allow {dir:?} for mod images: {e}");
                }
            }

            // NSIS/deb register the `ddmm://` scheme at install time (from
            // tauri.conf.json); a portable Windows exe or an AppImage has
            // no installer to do that, so register it ourselves every
            // launch too -- cheap and idempotent either way.
            let needs_runtime_scheme_registration = match decision.kind {
                data_dir::BaseDirKind::Portable => cfg!(windows),
                data_dir::BaseDirKind::AppData => false,
            } || std::env::var_os("APPIMAGE").is_some();
            if needs_runtime_scheme_registration {
                if let Err(e) = app.deep_link().register_all() {
                    log::warn!("Failed to register the ddmm:// scheme: {e}");
                }
            }

            // On Windows/Linux (unlike macOS/iOS) the deep-link plugin
            // doesn't scan argv on its own; a cold start via a `ddmm://`
            // link needs this explicit check. A second-instance launch is
            // instead caught by tauri-plugin-single-instance (registered
            // above, with its `deep-link` feature forwarding it into this
            // same `on_open_url`/`get_current` machinery).
            app.deep_link().handle_cli_arguments(std::env::args());
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                deep_link::handle(app.handle(), urls);
            }

            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                deep_link::handle(&handle, event.urls());
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = bridge::server::start(handle.clone()).await {
                    log::error!("Failed to start the browser bridge: {e}");
                }

                let exe = bridge::exe_path();
                let base_path = handle.state::<AppState>().base_path.clone();
                for outcome in bridge::native_messaging::register_all(&exe, &base_path).await {
                    if outcome.registered {
                        log::debug!("Native messaging registered for {}: {}", outcome.browser_id, outcome.detail);
                    } else {
                        log::warn!(
                            "Native messaging registration skipped for {}: {}",
                            outcome.browser_id,
                            outcome.detail
                        );
                    }
                }
            });

            auto_import::spawn(app.handle().clone());
            commands::updates::spawn_auto_check(app.handle().clone());

            Ok(())
        })
        // Last-resort safety net: guarantee the app can never be stranded
        // open by a broken close handler, regardless of *why* it's broken
        // (a future permission regression, the frontend crashing, the IPC
        // bridge itself being wedged, ...). The frontend's close handler
        // acks via `ack_close_requested` as the very first thing it does;
        // if that ack never arrives within a few seconds of a close being
        // requested, assume the frontend isn't going to handle this on its
        // own and force the process to exit directly. This never fires
        // during normal operation -- a real save, a confirmation popup
        // waiting on the user, etc. all happen *after* the ack, so they're
        // never affected by it.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Closing mid-move would be safe (the old data stays until
                // the new copy is complete) but would waste the copy and
                // leave a half-finished folder to clean up; the progress
                // popup asks the user to wait instead.
                let moving = window.app_handle().state::<AppState>().data_move_running.load(Ordering::SeqCst);
                if !app_lifecycle::close_allowed(moving) {
                    log::info!("Close requested while the data folder is being moved; ignoring it.");
                    api.prevent_close();
                    return;
                }
                let app_handle = window.app_handle().clone();
                let label = window.label().to_string();
                app_handle.state::<AppState>().close_ack.store(false, Ordering::SeqCst);
                tauri::async_runtime::spawn(async move {
                    const ACK_TIMEOUT: Duration = Duration::from_secs(10);
                    tokio::time::sleep(ACK_TIMEOUT).await;

                    let state = app_handle.state::<AppState>();
                    let acked = state.close_ack.load(Ordering::SeqCst);
                    let moving = state.data_move_running.load(Ordering::SeqCst);
                    let still_open = app_handle.get_webview_window(&label).is_some();
                    if moving && still_open && !acked {
                        log::info!("Close watchdog: a data folder move is running; not forcing exit.");
                    }
                    if app_lifecycle::watchdog_should_force_exit(still_open, acked, moving) {
                        log::warn!(
                            "Close watchdog: window '{label}' got no acknowledgement from the \
                             frontend within {ACK_TIMEOUT:?} of a close request; forcing exit."
                        );
                        app_handle.exit(0);
                    }
                });
            }
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::mods::get_mods,
            commands::mods::resolve_mod_image,
            commands::mods::delete_mod,
            commands::mods::add_mod,
            commands::mods::add_mods,
            commands::mods::add_mod_folder,
            commands::mods::add_paths,
            commands::mods::add_mod_from_url,
            commands::import::detect_import_sources,
            commands::import::scan_import_folder,
            commands::import::scan_import_paths,
            commands::import::run_import,
            commands::import::cancel_import,
            commands::handoff::start_handoff,
            commands::handoff::cancel_handoff,
            commands::handoff::install_handoff_file,
            commands::handoff::classify_download_url,
            commands::updates::check_updates,
            commands::updates::get_last_update_report,
            commands::updates::skip_update_version,
            commands::updates::update_mod_direct,
            commands::updates::browser_extension_active,
            commands::nexus::get_nexus_key_status,
            commands::nexus::set_nexus_api_key,
            commands::nexus::remove_nexus_api_key,
            commands::nexus::get_nexus_sign_in_status,
            commands::nexus::nexus_sign_in,
            commands::nexus::nexus_cancel_sign_in,
            commands::nexus::nexus_sign_out,
            commands::profiles::load_profiles,
            commands::profiles::save_profiles,
            commands::settings::load_settings,
            commands::settings::save_settings,
            commands::settings::check_settings,
            commands::settings::get_data_dir,
            commands::data_folder::get_data_folder_info,
            commands::data_folder::plan_data_folder_move,
            commands::data_folder::move_data_folder,
            commands::data_folder::adopt_data_folder,
            commands::data_folder::retry_data_folder,
            commands::data_folder::locate_data_folder,
            commands::data_folder::reset_data_folder_location,
            commands::settings::detect_game_path,
            commands::settings::validate_game_path,
            commands::settings::auto_detect_and_save_game_path,
            commands::purge,
            commands::deploy,
            commands::bridge::resolve_bridge_consent,
            commands::bridge::resolve_bridge_install_completion,
            commands::bridge::get_bridge_allowed_sites,
            commands::bridge::revoke_bridge_site,
            commands::bridge::repair_browser_integration,
            commands::bridge::remove_browser_integration,
            commands::bridge::repair_browser_integration_one,
            commands::bridge::remove_browser_integration_one,
            commands::bridge::focus_main_window,
            commands::bridge::is_game_running,
            commands::bridge::set_bridge_frontend_ready,
            commands::force_exit,
            commands::ack_close_requested,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            // Any exit (last window closed, the close watchdog, the
            // frontend's force-exit fallback) waits while a data folder
            // move is copying. See `app_lifecycle`.
            tauri::RunEvent::ExitRequested { api, .. } => {
                let moving = app_handle.state::<AppState>().data_move_running.load(Ordering::SeqCst);
                if !app_lifecycle::exit_allowed(moving) {
                    log::warn!("Exit requested while the data folder is being moved; ignoring it.");
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::Exit => {
                tauri::async_runtime::block_on(bridge::server::shutdown(app_handle));
                // Plugins have already handled `Exit` by now (Tauri runs them
                // before this callback), so the single-instance lock is
                // released and the new process won't hand off to this one.
                if app_handle.state::<AppState>().relaunch_on_exit.load(Ordering::SeqCst) {
                    let relaunch = app_lifecycle::relaunch_command(
                        &std::env::current_exe().unwrap_or_default(),
                        std::env::var_os("APPIMAGE").as_deref(),
                        &std::env::args_os().collect::<Vec<_>>(),
                    );
                    log::info!("Relaunching {:?} (no arguments) for the new data folder.", relaunch.program);
                    if let Err(e) = app_lifecycle::spawn_relaunch(&relaunch) {
                        log::error!("Couldn't relaunch DDMM ({:?}): {e}. Start it again by hand.", relaunch.program);
                    }
                }
            }
            _ => {}
        });
}