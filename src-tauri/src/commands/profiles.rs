use anyhow_tauri::{IntoTAResult, TAResult};
use tauri::State;

use crate::{AppState, models::profile::{Profile, ProfilesConfig}};

const PROFILES_FILE: &'static str = "profiles.json";

pub async fn do_load_profiles(base_path: &std::path::Path) -> anyhow::Result<ProfilesConfig> {
    let profiles_file = base_path.join(PROFILES_FILE);

    log::info!("Loading profiles...");

    log::info!("Looking for {:?}", &profiles_file);
    let profiles = if tokio::fs::try_exists(&profiles_file).await? {
        log::info!("Opening...");
        let data = tokio::fs::read(profiles_file).await?;

        log::info!("Deserializing...");
        serde_json::from_slice(&data)?
    } else {
        log::info!("Not found. Using default.");

        ProfilesConfig {
            profiles: vec![
                Profile::V1 {
                    name: "Default".to_string(),
                    configs: Vec::new()
                }
            ],
            active: 0
        }
    };

    log::info!("Profiles loaded.");
    Ok(profiles)
}

#[tauri::command]
pub async fn load_profiles(state: State<'_, AppState>) -> TAResult<ProfilesConfig> {
    do_load_profiles(&state.base_path).await.into_ta_result()
}

#[tauri::command]
pub async fn save_profiles(state: State<'_, AppState>, config: ProfilesConfig) -> TAResult<()> {
    log::info!("Saving profiles...");
    
    if log::log_enabled!(log::Level::Debug) {
        for profile in &config.profiles {
            log::debug!("Profile \"{}\" Order:", profile.name());
            for config in profile.configs() {
                log::debug!("- {{{}}}", config.uuid());
            }
        }
    }

    let data = serde_json::to_vec_pretty(&config).into_ta_result()?;
    tokio::fs::write(state.base_path.join(PROFILES_FILE), data).await.into_ta_result()?;
    
    log::info!("Profiles saved.");

    Ok(())
}