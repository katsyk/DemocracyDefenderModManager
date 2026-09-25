//! Frontend-facing commands for the browser bridge: resolving the two
//! oneshot round trips `bridge::server` starts (consent, install
//! completion), managing the "Always allow" site list, and browser
//! integration (native messaging) status/repair/remove for Settings.

use anyhow_tauri::{IntoTAResult, TAResult};
use tauri::{AppHandle, State};

use crate::{
    bridge::{native_messaging, protocol::ErrorCode, ConsentDecision, InstallCompletion},
    commands::settings::do_load_settings,
    AppState,
};

const SETTINGS_FILE: &str = "settings.json";

/// Resolve a pending `bridge://consent-request` -- the frontend calls this
/// after the user clicks Always allow / Just this once / Deny.
#[tauri::command]
pub async fn resolve_bridge_consent(state: State<'_, AppState>, request_id: String, decision: String) -> TAResult<()> {
    let decision = match decision.as_str() {
        "AlwaysAllow" => ConsentDecision::AlwaysAllow,
        "JustOnce" => ConsentDecision::JustOnce,
        "Deny" => ConsentDecision::Deny,
        other => return anyhow::anyhow!("unknown consent decision \"{other}\"").into_ta_result(),
    };

    let sender = state.bridge_pending.lock().await.consent.remove(&request_id);
    if let Some(sender) = sender {
        // The receiver may already have timed out; nothing to do either way.
        let _ = sender.send(decision);
    }
    Ok(())
}

/// Resolve a pending `bridge://mod-installed` -- the frontend calls this
/// after it has added the mod to a profile and/or deployed, per
/// `afterInstall`.
#[tauri::command]
pub async fn resolve_bridge_install_completion(
    state: State<'_, AppState>,
    request_id: String,
    added_to_profile: Option<String>,
    deployed: bool,
    warnings: Vec<String>,
    soft_error_code: Option<String>,
    soft_error_message: Option<String>,
) -> TAResult<()> {
    let soft_error = match soft_error_code.as_deref() {
        Some("GAME_NOT_FOUND") => Some((ErrorCode::GameNotFound, soft_error_message.unwrap_or_default())),
        Some("DEPLOY_FAILED") => Some((ErrorCode::DeployFailed, soft_error_message.unwrap_or_default())),
        Some(other) => return anyhow::anyhow!("unknown soft error code \"{other}\"").into_ta_result(),
        None => None,
    };

    let completion = InstallCompletion {
        added_to_profile,
        deployed,
        warnings,
        soft_error,
    };

    let sender = state.bridge_pending.lock().await.install_completion.remove(&request_id);
    if let Some(sender) = sender {
        let _ = sender.send(completion);
    }
    Ok(())
}

/// Sites the user has clicked "Always allow" for -- shown, revocably, in
/// Settings.
#[tauri::command]
pub async fn get_bridge_allowed_sites(state: State<'_, AppState>) -> TAResult<Vec<String>> {
    let settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    Ok(settings.bridge_allowed_sites().to_vec())
}

#[tauri::command]
pub async fn revoke_bridge_site(state: State<'_, AppState>, site: String) -> TAResult<()> {
    let mut settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    settings.revoke_bridge_site(&site);
    let data = serde_json::to_vec_pretty(&settings).into_ta_result()?;
    tokio::fs::write(state.base_path.join(SETTINGS_FILE), data).await.into_ta_result()?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BrowserIntegrationStatus {
    pub browser_id: &'static str,
    pub display_name: &'static str,
    pub registered: bool,
    pub detail: String,
}

/// Re-register every browser's native messaging manifest (idempotent --
/// always overwrites) and report per-browser status. Used both at startup
/// and as Settings' "Repair" button.
#[tauri::command]
pub async fn repair_browser_integration(state: State<'_, AppState>) -> TAResult<Vec<BrowserIntegrationStatus>> {
    Ok(run_registration(&state).await)
}

pub async fn run_registration(state: &AppState) -> Vec<BrowserIntegrationStatus> {
    let exe = crate::bridge::exe_path();
    let outcomes = native_messaging::register_all(&exe, &state.base_path).await;

    outcomes
        .into_iter()
        .map(|o| {
            let target = native_messaging::BROWSERS.iter().find(|b| b.id == o.browser_id);
            BrowserIntegrationStatus {
                browser_id: o.browser_id,
                display_name: target.map(|t| t.display_name).unwrap_or("unknown"),
                registered: o.registered,
                detail: o.detail,
            }
        })
        .collect()
}

/// Remove every native messaging manifest DDMM registered (Settings'
/// "Remove all" button / portable & Linux uninstall path -- NSIS handles
/// Windows registry cleanup itself on uninstall).
#[tauri::command]
pub async fn remove_browser_integration(state: State<'_, AppState>) -> TAResult<()> {
    native_messaging::remove_all(&state.base_path).await;
    Ok(())
}

/// Re-register (or first-time register) just one browser -- Settings'
/// per-browser "Repair" button.
#[tauri::command]
pub async fn repair_browser_integration_one(
    state: State<'_, AppState>,
    browser_id: String,
) -> TAResult<Option<BrowserIntegrationStatus>> {
    let exe = crate::bridge::exe_path();
    let Some(outcome) = native_messaging::register_one_by_id(&browser_id, &exe, &state.base_path).await else {
        return Ok(None);
    };
    let target = native_messaging::BROWSERS.iter().find(|b| b.id == outcome.browser_id);
    Ok(Some(BrowserIntegrationStatus {
        browser_id: outcome.browser_id,
        display_name: target.map(|t| t.display_name).unwrap_or("unknown"),
        registered: outcome.registered,
        detail: outcome.detail,
    }))
}

/// Remove just one browser's manifest -- Settings' per-browser "Remove"
/// button.
#[tauri::command]
pub async fn remove_browser_integration_one(state: State<'_, AppState>, browser_id: String) -> TAResult<()> {
    native_messaging::remove_one(&browser_id, &state.base_path).await;
    Ok(())
}

/// Cheap check for whether Helldivers 2 is currently running, so the
/// frontend can refuse `afterInstall: deploy` for a browser install rather
/// than overwrite a running game's data files. See `steam::is_game_running`.
#[tauri::command]
pub fn is_game_running() -> bool {
    crate::steam::is_game_running()
}

/// Bring the main window to front on demand -- `deep_link` and
/// `bridge::server` use the same underlying helper for the same reason
/// (a `ddmm://open` link, or a browser install prompt); this command lets
/// Settings' "Get the extension" flow request it too.
#[tauri::command]
pub fn focus_main_window(app: AppHandle) {
    crate::bridge::server::focus_main_window(&app);
}
