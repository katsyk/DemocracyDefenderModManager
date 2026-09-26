//! Settings -> Nexus Mods: the optional "Sign in to Nexus Mods" (OAuth, see
//! `crate::nexus_oauth`) and the optional personal API key entered
//! manually (from the user's Nexus account API Keys page).
//!
//! If both are set, the sign-in is used (see
//! `crate::nexus_oauth::resolve_auth`).
//!
//! The key is only used for
//! update checks; see `crate::secrets` for how it's stored and
//! `crate::providers::nexus` for how it's used. It never reaches the
//! frontend again once saved (only the account name and where it's stored
//! do), never goes into settings.json, and is never logged.

use anyhow_tauri::{IntoTAResult, TAResult};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::{
    commands::settings::{do_load_settings, write_settings},
    nexus_oauth::{self, SignInConfig, SignInError},
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
    let _data_op = state.data_op().into_ta_result()?;
    let key = match NexusApiKey::parse(&key) {
        Ok(k) => k,
        Err(reason) => return anyhow::anyhow!("That doesn't look like a Nexus Mods API key: {reason}.").into_ta_result(),
    };

    validate_and_store(&state, key).await.into_ta_result()
}

/// Check `key` with Nexus (`/v1/users/validate.json`, a user-initiated
/// action) and, only if Nexus accepts it, store it and remember the
/// account name. Errors never contain the key.
pub async fn validate_and_store(state: &AppState, key: NexusApiKey) -> anyhow::Result<NexusKeyStatus> {
    log::info!("Validating a Nexus Mods API key with api.nexusmods.com...");
    let mut client = NexusClient::new(key.clone().into())?;
    let user = match client.validate().await {
        Ok(user) => user,
        Err(NexusError::InvalidKey) => {
            log::warn!("Nexus Mods rejected the API key.");
            anyhow::bail!("Nexus Mods didn't accept that key. Copy it again from your Nexus Mods account (API Keys), then paste it here.");
        }
        Err(e) => {
            let message = secrets::redact(&e.to_string(), &key);
            log::warn!("Couldn't validate the Nexus Mods API key: {message}");
            anyhow::bail!("Couldn't check the key with Nexus Mods: {message}");
        }
    };

    let storage = secrets::store(&state.base_path, &key)
        .await
        .map_err(|e| anyhow::anyhow!(secrets::redact(&e.to_string(), &key)))?;

    let mut settings = do_load_settings(&state.base_path).await?;
    settings.set_nexus_username(Some(user.name.clone()));
    write_settings(&state.base_path, &settings).await?;

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
    let _data_op = state.data_op().into_ta_result()?;
    secrets::remove(&state.base_path).await;
    let _ = tokio::fs::remove_file(state.base_path.join("update-cache.json")).await;
    let mut settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    settings.set_nexus_username(None);
    write_settings(&state.base_path, &settings).await.into_ta_result()?;
    log::info!("Nexus Mods API key removed.");
    Ok(NexusKeyStatus { present: false, storage: None, username: None, is_premium: None })
}

/// Settings' view of the optional Nexus Mods sign-in. Never carries a
/// token -- only whether sign-in is possible, the account name, and where
/// the sign-in is stored.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NexusSignInStatus {
    /// This build has a Nexus OAuth client ID (else: "Coming soon").
    pub available: bool,
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<KeyStorage>,
    /// Port the sign-in listens on (shown if it's busy).
    pub port: u16,
}

async fn sign_in_status(state: &AppState) -> NexusSignInStatus {
    let stored = nexus_oauth::load_tokens(&state.base_path).await;
    NexusSignInStatus {
        available: nexus_oauth::client_id().is_some(),
        signed_in: stored.is_some(),
        username: stored.as_ref().and_then(|(t, _)| t.username.clone()),
        storage: stored.map(|(_, s)| s),
        port: nexus_oauth::REDIRECT_PORT,
    }
}

#[tauri::command]
pub async fn get_nexus_sign_in_status(state: State<'_, AppState>) -> TAResult<NexusSignInStatus> {
    Ok(sign_in_status(&state).await)
}

/// "Sign in to Nexus Mods": opens the system browser and waits (up to 5
/// minutes) for the user to approve. User-initiated only.
#[tauri::command]
pub async fn nexus_sign_in(app: AppHandle, state: State<'_, AppState>) -> TAResult<NexusSignInStatus> {
    let _data_op = state.data_op().into_ta_result()?;
    let Some(client_id) = nexus_oauth::client_id() else {
        return anyhow::anyhow!(SignInError::NoClientId.to_string()).into_ta_result();
    };

    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    {
        let mut slot = state.nexus_sign_in_cancel.lock().await;
        if slot.as_ref().is_some_and(|tx| !tx.is_closed()) {
            return anyhow::anyhow!(SignInError::AlreadyRunning.to_string()).into_ta_result();
        }
        *slot = Some(cancel_tx);
    }

    log::info!("Starting Nexus Mods sign-in.");
    let cfg = SignInConfig::production(client_id);
    let opener = app.clone();
    let result = nexus_oauth::sign_in(
        &cfg,
        // The authorize URL carries no secret (only the PKCE challenge and
        // `state`); it goes to the system browser, never the bridge.
        move |url| opener.opener().open_url(url, None::<&str>).map_err(|e| e.to_string()),
        cancel_rx,
    )
    .await;
    state.nexus_sign_in_cancel.lock().await.take();

    let tokens = match result {
        Ok(tokens) => tokens,
        Err(e) => {
            log::warn!("Nexus Mods sign-in didn't complete: {e}");
            return anyhow::anyhow!(e.to_string()).into_ta_result();
        }
    };
    let storage = match nexus_oauth::store_tokens(&state.base_path, &tokens).await {
        Ok(s) => s,
        Err(e) => {
            let e = SignInError::Storage(e.to_string());
            log::warn!("{e}");
            return anyhow::anyhow!(e.to_string()).into_ta_result();
        }
    };
    // Cached Nexus decisions belong to whatever account made them.
    let _ = tokio::fs::remove_file(state.base_path.join("update-cache.json")).await;
    log::info!(
        "Signed in to Nexus Mods as \"{}\" ({storage:?}).",
        tokens.username.as_deref().unwrap_or("(unknown)")
    );
    Ok(sign_in_status(&state).await)
}

/// Stop waiting for the browser.
#[tauri::command]
pub async fn nexus_cancel_sign_in(state: State<'_, AppState>) -> TAResult<()> {
    if let Some(tx) = state.nexus_sign_in_cancel.lock().await.take() {
        let _ = tx.send(());
        log::info!("Nexus Mods sign-in cancelled.");
    }
    Ok(())
}

/// "Sign out": ask Nexus to revoke the sign-in (best effort), and always
/// delete it from this computer.
#[tauri::command]
pub async fn nexus_sign_out(state: State<'_, AppState>) -> TAResult<NexusSignInStatus> {
    let _data_op = state.data_op().into_ta_result()?;
    if let Some((tokens, _)) = nexus_oauth::load_tokens(&state.base_path).await {
        if let Some(client_id) = nexus_oauth::client_id() {
            nexus_oauth::revoke(&nexus_oauth::Endpoints::nexus(), &client_id, &tokens).await;
        }
    }
    nexus_oauth::remove_tokens(&state.base_path).await;
    log::info!("Signed out of Nexus Mods.");
    Ok(sign_in_status(&state).await)
}
