//! Settings' optional "Nexus Mods API key" field. The key is only used for
//! update checks; see `crate::secrets` for how it's stored and
//! `crate::providers::nexus` for how it's used. It never reaches the
//! frontend again once saved (only the account name and where it's stored
//! do), never goes into settings.json, and is never logged.

use anyhow_tauri::{IntoTAResult, TAResult};
use serde::Serialize;
use tauri::State;

use crate::{
    commands::settings::{do_load_settings, write_settings},
    providers::nexus::{NexusClient, NexusError},
    secrets::{self, KeyStorage, NexusApiKey},
    AppState,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NexusKeyStatus {
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<KeyStorage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_premium: Option<bool>,
}

#[tauri::command]
pub async fn get_nexus_key_status(state: State<'_, AppState>) -> TAResult<NexusKeyStatus> {
    let stored = secrets::load(&state.base_path).await;
    let username = do_load_settings(&state.base_path)
        .await
        .ok()
        .and_then(|s| s.nexus_username().map(str::to_string));
    Ok(NexusKeyStatus {
        present: stored.is_some(),
        storage: stored.map(|(_, storage)| storage),
        username,
        is_premium: None,
    })
}

/// Validate a pasted key with Nexus (`/v1/users/validate.json`), and only
/// if Nexus accepts it, store it (OS keychain, or the Linux fallback file).
/// Errors never contain the key.
#[tauri::command]
pub async fn set_nexus_api_key(state: State<'_, AppState>, key: String) -> TAResult<NexusKeyStatus> {
    let key = match NexusApiKey::parse(&key) {
        Ok(k) => k,
        Err(reason) => return anyhow::anyhow!("That doesn't look like a Nexus Mods API key: {reason}.").into_ta_result(),
    };

    log::info!("Validating a Nexus Mods API key with api.nexusmods.com...");
    let mut client = NexusClient::new(key.clone()).into_ta_result()?;
    let user = match client.validate().await {
        Ok(user) => user,
        Err(NexusError::InvalidKey) => {
            log::warn!("Nexus Mods rejected the API key.");
            return anyhow::anyhow!("Nexus Mods didn't accept that key. Copy it again from your Nexus Mods account (API Keys), then paste it here.")
                .into_ta_result();
        }
        Err(e) => {
            let message = secrets::redact(&e.to_string(), &key);
            log::warn!("Couldn't validate the Nexus Mods API key: {message}");
            return anyhow::anyhow!("Couldn't check the key with Nexus Mods: {message}").into_ta_result();
        }
    };

    let storage = secrets::store(&state.base_path, &key)
        .await
        .map_err(|e| anyhow::anyhow!(secrets::redact(&e.to_string(), &key)))
        .into_ta_result()?;

    let mut settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    settings.set_nexus_username(Some(user.name.clone()));
    write_settings(&state.base_path, &settings).await.into_ta_result()?;

    // Cached Nexus decisions belong to whatever key/account made them;
    // start fresh.
    let _ = tokio::fs::remove_file(state.base_path.join("update-cache.json")).await;

    log::info!("Nexus Mods API key saved for account \"{}\" ({:?}).", user.name, storage);
    Ok(NexusKeyStatus {
        present: true,
        storage: Some(storage),
        username: Some(user.name),
        is_premium: Some(user.is_premium),
    })
}

/// Forget the key everywhere it could be stored.
#[tauri::command]
pub async fn remove_nexus_api_key(state: State<'_, AppState>) -> TAResult<NexusKeyStatus> {
    secrets::remove(&state.base_path).await;
    let _ = tokio::fs::remove_file(state.base_path.join("update-cache.json")).await;
    let mut settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    settings.set_nexus_username(None);
    write_settings(&state.base_path, &settings).await.into_ta_result()?;
    log::info!("Nexus Mods API key removed.");
    Ok(NexusKeyStatus { present: false, storage: None, username: None, is_premium: None })
}
