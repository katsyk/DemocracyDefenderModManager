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
    let _data_op = state.data_op().into_ta_result()?;
    log::info!("Saving profiles...");
    
    if log::log_enabled!(log::Level::Debug) {
        for profile in &config.profiles {
            log::debug!("Profile \"{}\" Order:", profile.name());
            for config in profile.configs() {
                log::debug!("- {{{}}}", config.uuid());
            }
        }
    }

    let _saving = PROFILES_WRITE.lock().await;
    write_profiles(&state.base_path, &config).await.into_ta_result()?;

    log::info!("Profiles saved.");

    Ok(())
}

/// Take every entry of the mod `guid` out of every profile in the saved
/// profiles.json, and return how many there were. Called when a mod is
/// deleted, so that a deleted mod never comes back as "Mod not found" --
/// even if DDMM is closed before the frontend saves its profiles again.
/// Nothing is written when no profile has it (or there's no file yet).
pub async fn remove_from_saved_profiles(base_path: &std::path::Path, guid: uuid::Uuid) -> anyhow::Result<usize> {
    let _saving = PROFILES_WRITE.lock().await;
    if !tokio::fs::try_exists(base_path.join(PROFILES_FILE)).await? {
        return Ok(0);
    }
    let mut config = do_load_profiles(base_path).await?;
    let mut removed = 0;
    for profile in &mut config.profiles {
        match profile {
            Profile::V1 { configs, .. } => {
                let before = configs.len();
                configs.retain(|c| *c.uuid() != guid);
                removed += before - configs.len();
            }
        }
    }
    if removed > 0 {
        write_profiles(base_path, &config).await?;
    }
    Ok(removed)
}

/// Held while profiles.json is written (or read to be rewritten), so a
/// save from the page and a delete's own update of the file never
/// interleave.
static PROFILES_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Write profiles.json atomically: into a temporary file next to it, then
/// renamed over it, so it's never seen (or left, on a crash) half written.
async fn write_profiles(base_path: &std::path::Path, config: &ProfilesConfig) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(config)?;
    let file = base_path.join(PROFILES_FILE);
    let temp = base_path.join(format!("{PROFILES_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let written = async {
        tokio::fs::write(&temp, &data).await?;
        tokio::fs::rename(&temp, &file).await
    }
    .await;
    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(anyhow::Error::from(e).context(format!("couldn't write {:?}", file)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles_with(guids: &[&str]) -> ProfilesConfig {
        let configs = guids
            .iter()
            .map(|g| serde_json::json!({ "For": "V1", "Guid": g, "Enabled": true, "Toggled": [], "Selected": [] }))
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({ "Profiles": [{ "Version": "V1", "Name": "Default", "Configs": configs }], "Active": 0 }))
            .unwrap()
    }

    const A: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
    const B: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

    /// Saves and a delete's removal running at once (all without the
    /// deleted mod, as the page's saves are once the delete returned):
    /// the result is a whole, valid file without it, and no temporary file
    /// is left.
    #[tokio::test]
    async fn concurrent_writes_leave_a_whole_file_and_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        write_profiles(dir.path(), &profiles_with(&[A, B])).await.unwrap();
        let mut tasks = Vec::new();
        for i in 0..20 {
            let base = dir.path().to_path_buf();
            tasks.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    let _saving = PROFILES_WRITE.lock().await;
                    write_profiles(&base, &profiles_with(&[A])).await.unwrap();
                } else {
                    remove_from_saved_profiles(&base, uuid::Uuid::parse_str(B).unwrap()).await.unwrap();
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let loaded = do_load_profiles(dir.path()).await.unwrap();
        let guids: Vec<String> = loaded.profiles[0].configs().iter().map(|c| c.uuid().to_string()).collect();
        assert_eq!(guids, vec![A.to_string()]);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![PROFILES_FILE.to_string()]);
    }

    #[tokio::test]
    async fn removing_from_profiles_without_a_file_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(remove_from_saved_profiles(dir.path(), uuid::Uuid::parse_str(A).unwrap()).await.unwrap(), 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
