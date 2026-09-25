pub mod commands;
pub mod models;
pub mod archive;
pub mod utils;
pub mod sources;
pub mod download;
pub mod data_dir;
pub mod steam;

use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

use log::LevelFilter;
use tauri_plugin_log::{Target, TargetKind};
use tokio::sync::Mutex;

use crate::models::Mod;

pub struct AppState {
    base_path: PathBuf,
    mods: Mutex<Option<Vec<Mod>>>,
    /// Cancellation flag for the single in-flight browser handoff, if any.
    /// `commands::handoff` is the only thing that touches this.
    handoff_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            base_path,
            mods: Mutex::default(),
            handoff_cancel: Mutex::default(),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
        .setup(move |_app| {
            log::info!("{}", startup_message);
            Ok(())
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
            commands::force_exit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}