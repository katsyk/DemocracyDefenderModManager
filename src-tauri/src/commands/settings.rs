use std::path::{Path, PathBuf};

use anyhow_tauri::{IntoTAResult, TAResult};
use tauri::State;

use crate::{AppState, models::settings::Settings};

const SETTINGS_FILE: &'static str = "settings.json";

/// The OS Downloads directory, if one can be determined. Used as the
/// default `downloads_path` for the browser handoff (`commands::handoff`).
pub fn default_downloads_path() -> PathBuf {
    dirs::download_dir().unwrap_or_default()
}

pub async fn do_load_settings(base_path: &Path) -> anyhow::Result<Settings> {
    log::info!("Loading settings...");
    let settings_file = base_path.join(SETTINGS_FILE);

    log::info!("Looking for {:?}", &settings_file);
    let mut settings = if tokio::fs::try_exists(&settings_file).await? {
        log::info!("Opening...");
        let data = tokio::fs::read(&settings_file).await?;

        log::info!("Deserializing...");
        serde_json::from_slice(&data)?
    } else {
        log::info!("Not found. Using default.");

        Settings::V1 {
            game_path: PathBuf::new(),
            skip_list: vec![],
            downloads_path: default_downloads_path(),
        }
    };

    // Settings files written before `downloads_path` existed deserialize it
    // as an empty path (`#[serde(default)]`); fill in the OS default rather
    // than leaving it unusable.
    if settings.downloads_path().as_os_str().is_empty() {
        settings.set_downloads_path(default_downloads_path());
    }

    log::info!("Settings loaded.");
    Ok(settings)
}

pub async fn do_check_settings(base_path: &Path) -> anyhow::Result<bool> {
    log::info!("Checking settings...");

    match do_load_settings(base_path).await {
        Ok(settings) => {
            match settings.validate().await {
                Ok(()) => {
                    log::info!("Settings vaid.");
                    Ok(true)
                }
                Err(e) => {
                    log::error!("Settings invalid: {}", e);       
                    Ok(false)
                }
            }
        }
        Err(e) => Err(e)
    }
}

#[tauri::command]
pub async fn load_settings(state: State<'_, AppState>) -> TAResult<Settings> {
    do_load_settings(&state.base_path).await.into_ta_result()
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Settings) -> TAResult<()> {
    log::info!("Saving settings...");

    let data = serde_json::to_vec_pretty(&settings).into_ta_result()?;
    tokio::fs::write(state.base_path.join(SETTINGS_FILE), data).await.into_ta_result()?;

    log::info!("Settings saved.");
    Ok(())
}

#[tauri::command]
pub  async fn check_settings(state: State<'_, AppState>) -> TAResult<bool> {
    do_check_settings(&state.base_path).await.into_ta_result()
}

/// The directory DDMM is currently keeping `mods/`, `settings.json`,
/// `profiles.json` and logs in -- shown read-only in Settings with an
/// "Open folder" button.
#[tauri::command]
pub fn get_data_dir(state: State<'_, AppState>) -> String {
    state.base_path.to_string_lossy().into_owned()
}

/// Look for a Helldivers 2 install via Steam, without touching settings.
/// Backs the "Auto-detect" button in Settings.
#[tauri::command]
pub async fn detect_game_path() -> Option<String> {
    crate::steam::detect_game_path()
        .await
        .map(|p| p.to_string_lossy().into_owned())
}

/// Detect a Helldivers 2 install and, if found, save it as the game path
/// immediately. Used on first run (and whenever settings are otherwise
/// invalid) so a new player never has to open Settings by hand. Returns the
/// detected path, if any, so the caller can tell the user where it found
/// it.
#[tauri::command]
pub async fn auto_detect_and_save_game_path(state: State<'_, AppState>) -> TAResult<Option<String>> {
    let Some(path) = crate::steam::detect_game_path().await else {
        return Ok(None);
    };

    let mut settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    settings.set_game_path(path.clone());

    let data = serde_json::to_vec_pretty(&settings).into_ta_result()?;
    tokio::fs::write(state.base_path.join(SETTINGS_FILE), data).await.into_ta_result()?;

    Ok(Some(path.to_string_lossy().into_owned()))
}