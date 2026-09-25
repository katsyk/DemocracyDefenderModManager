pub mod commands;
pub mod models;
pub mod archive;
pub mod utils;
pub mod sources;
pub mod download;
pub mod data_dir;
pub mod steam;
pub mod bridge;
pub mod deep_link;
pub mod auto_import;

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
use tauri_plugin_log::{Target, TargetKind};
use tokio::sync::Mutex;

use crate::{bridge::BridgePending, models::Mod};

pub struct AppState {
    base_path: PathBuf,
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
}

impl AppState {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            base_path,
            mods: Mutex::default(),
            handoff_cancel: Mutex::default(),
            bridge_pending: Mutex::default(),
            bridge_queue_depth: AtomicUsize::new(0),
            bridge_install_lock: Mutex::new(()),
            bridge_frontend_ready: tokio::sync::watch::Sender::new(false),
            close_ack: AtomicBool::new(false),
        }
    }
}

/// Compute the base data directory the same way the desktop app does,
/// without touching Tauri at all -- used both by `run()` before the
/// builder is constructed and by host mode, which never constructs one.
fn resolve_base_path() -> PathBuf {
    let exe_dir = data_dir::resolve_exe_dir();
    let app_data_dir = data_dir::platform_app_data_dir().unwrap_or_else(|e| {
        eprintln!("warning: {e}; falling back to the executable directory for app data");
        exe_dir.clone()
    });
    data_dir::decide_base_dir(&exe_dir, &app_data_dir).path
}

/// Run as the browser's native-messaging host: relay only, no window, no
/// Tauri runtime at all. Must be checked before anything Tauri-related is
/// touched, since a host-mode invocation has no display to attach to.
fn run_host_mode(origin: bridge::host::HostOrigin) {
    let base_path = resolve_base_path();
    let _ = std::fs::create_dir_all(&base_path);

    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start host-mode runtime: {e}");
            return;
        }
    };
    runtime.block_on(bridge::host::run(origin, base_path));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(origin) = bridge::host::detect_host_mode(&args) {
        run_host_mode(origin);
        return;
    }

    let exe_dir = data_dir::resolve_exe_dir();
    let app_data_dir = data_dir::platform_app_data_dir().unwrap_or_else(|e| {
        eprintln!("warning: {e}; falling back to the executable directory for app data");
        exe_dir.clone()
    });

    let decision = data_dir::decide_base_dir(&exe_dir, &app_data_dir);
    if let Err(e) = std::fs::create_dir_all(&decision.path) {
        eprintln!("warning: failed to create data directory {:?}: {}", decision.path, e);
    }

    let base_path = decision.path.clone();
    let asset_base_path = base_path.clone();
    let log_dir = base_path.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    // The base-directory decision itself is logged again once the log
    // plugin (which needs this same `log_dir`) is up, so it actually lands
    // in the log file and not just stdout.
    let startup_message = format!(
        "Using {} directory: {:?} ({})",
        match decision.kind {
            data_dir::BaseDirKind::Portable => "portable data",
            data_dir::BaseDirKind::AppData => "app data",
        },
        base_path,
        decision.reason
    );

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

            // Mod icons and option images are shown with the asset protocol;
            // allow exactly the mods folder under the data folder, nothing
            // else (tauri.conf.json grants no static scope).
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
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let app_handle = window.app_handle().clone();
                let label = window.label().to_string();
                app_handle.state::<AppState>().close_ack.store(false, Ordering::SeqCst);
                tauri::async_runtime::spawn(async move {
                    const ACK_TIMEOUT: Duration = Duration::from_secs(10);
                    tokio::time::sleep(ACK_TIMEOUT).await;

                    let acked = app_handle.state::<AppState>().close_ack.load(Ordering::SeqCst);
                    let still_open = app_handle.get_webview_window(&label).is_some();
                    if still_open && !acked {
                        log::warn!(
                            "Close watchdog: window '{label}' got no acknowledgement from the \
                             frontend within {ACK_TIMEOUT:?} of a close request; forcing exit."
                        );
                        app_handle.exit(0);
                    }
                });
            }
        })
        .manage(AppState::new(base_path))
        .invoke_handler(tauri::generate_handler![
            commands::mods::get_mods,
            commands::mods::delete_mod,
            commands::mods::add_mod,
            commands::mods::add_mods,
            commands::mods::add_mod_folder,
            commands::mods::add_paths,
            commands::mods::add_mod_from_url,
            commands::handoff::start_handoff,
            commands::handoff::cancel_handoff,
            commands::handoff::install_handoff_file,
            commands::handoff::classify_download_url,
            commands::updates::check_updates,
            commands::profiles::load_profiles,
            commands::profiles::save_profiles,
            commands::settings::load_settings,
            commands::settings::save_settings,
            commands::settings::check_settings,
            commands::settings::get_data_dir,
            commands::settings::detect_game_path,
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
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                tauri::async_runtime::block_on(bridge::server::shutdown(app_handle));
            }
        });
}