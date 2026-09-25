//! The browser bridge: a loopback TCP relay that lets the DDMM browser
//! extension (via a native-messaging host running in the same `ddmm`
//! executable) hand off completed downloads for one-click install. See
//! `docs/development/bridge-protocol.md` for the full contract.

pub mod allowlist;
pub mod host;
pub mod install;
pub mod native_messaging;
pub mod protocol;
pub mod server;
pub mod state;

use std::{collections::HashMap, path::PathBuf};

use tokio::sync::oneshot;

/// The exe path used both to launch DDMM detached (from host mode) and to
/// point native-messaging manifests at: prefers `$APPIMAGE` over
/// `current_exe()` so a re-downloaded AppImage or a portable exe moved to a
/// new location keeps working without re-registering by hand.
pub fn exe_path() -> PathBuf {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return PathBuf::from(appimage);
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ddmm"))
}

/// How the user answered a per-site consent prompt ("Allow the DDMM
/// browser extension to install mods from **example.com**?").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    AlwaysAllow,
    JustOnce,
    Deny,
}

/// What the frontend did after the backend installed a mod on the bridge's
/// behalf (adding it to the active profile and/or deploying, per
/// `afterInstall`) -- reported back so the TCP handler can build the final
/// reply to the extension.
#[derive(Debug, Clone, Default)]
pub struct InstallCompletion {
    pub added_to_profile: Option<String>,
    pub deployed: bool,
    pub warnings: Vec<String>,
    /// Set when `afterInstall` asked for something (a profile, a deploy)
    /// that didn't fully succeed -- the mod itself is still installed
    /// either way, matching `GAME_NOT_FOUND`/`DEPLOY_FAILED`'s semantics
    /// in the protocol spec.
    pub soft_error: Option<(protocol::ErrorCode, String)>,
}

/// Oneshot channels the bridge server is waiting on, keyed by the bridge
/// request `id`, resolved by a frontend-invoked Tauri command once the
/// user (or the frontend's own profile/deploy logic) has answered.
#[derive(Default)]
pub struct BridgePending {
    pub consent: HashMap<String, oneshot::Sender<ConsentDecision>>,
    pub install_completion: HashMap<String, oneshot::Sender<InstallCompletion>>,
}
