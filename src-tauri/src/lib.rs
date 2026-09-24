pub mod commands;
pub mod models;
pub mod archive;
pub mod utils;
pub mod sources;
pub mod download;

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
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

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
                    //#[cfg(not(debug_assertions))]
                    Target::new(TargetKind::Folder {
                        path: exe_dir.clone(),
                        file_name: None
                    })
                ])
                .build()
        )
        .manage(AppState::new(exe_dir))
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
            commands::purge,
            commands::deploy
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}