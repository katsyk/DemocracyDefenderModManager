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

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

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

impl BridgePending {
    /// A fresh key for one consent/completion round trip. Never the
    /// extension's request `id`: that restarts at "1" whenever the browser
    /// restarts the extension, so a late answer meant for an older install
    /// could otherwise resolve a newer one.
    fn new_key() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// Start waiting for a consent answer: the key the frontend answers
    /// with, and where the answer arrives.
    pub fn wait_for_consent(&mut self) -> (String, oneshot::Receiver<ConsentDecision>) {
        let (tx, rx) = oneshot::channel();
        let key = Self::new_key();
        self.consent.insert(key.clone(), tx);
        (key, rx)
    }

    /// Start waiting for the frontend's afterInstall report.
    pub fn wait_for_completion(&mut self) -> (String, oneshot::Receiver<InstallCompletion>) {
        let (tx, rx) = oneshot::channel();
        let key = Self::new_key();
        self.install_completion.insert(key.clone(), tx);
        (key, rx)
    }
}

/// Downloads the browser extension has asked DDMM to install: while such an
/// install is running, and for [`RecentBridgeFiles::KEEP_AFTER`] after it
/// finished, auto-import (which watches the same Downloads folder) doesn't
/// offer the same file again -- DDMM is already installing it, and
/// "Install" would then only fail with "already installed".
#[derive(Default)]
pub struct RecentBridgeFiles(std::sync::Mutex<HashMap<PathBuf, Option<Instant>>>);

impl RecentBridgeFiles {
    pub const KEEP_AFTER: Duration = Duration::from_secs(10 * 60);

    /// The same file however it was spelled (`.`/`..`, symlinked folders,
    /// and on Windows, letter case).
    fn key(path: &Path) -> PathBuf {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if cfg!(windows) {
            PathBuf::from(path.to_string_lossy().to_lowercase())
        } else {
            path
        }
    }

    /// An install of `path` has started (and until [`Self::finish`], counts
    /// however long it takes: a consent prompt, a queue of installs).
    pub fn begin(&self, path: &Path) {
        let key = Self::key(path);
        if let Ok(mut map) = self.0.lock() {
            map.retain(|_, done| done.is_none_or(|t| t.elapsed() < Self::KEEP_AFTER));
            map.insert(key, None);
        }
    }

    /// The install of `path` finished (either way).
    pub fn finish(&self, path: &Path) {
        let key = Self::key(path);
        if let Ok(mut map) = self.0.lock() {
            map.insert(key, Some(Instant::now()));
        }
    }

    /// Whether `path` is being installed through the extension, or was
    /// within [`Self::KEEP_AFTER`].
    pub fn contains(&self, path: &Path) -> bool {
        let key = Self::key(path);
        self.0
            .lock()
            .ok()
            .and_then(|map| map.get(&key).copied())
            .is_some_and(|done| done.is_none_or(|t| t.elapsed() < Self::KEEP_AFTER))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A late answer to an older prompt (the frontend answering with the
    /// old key) can't resolve a newer install's prompt.
    #[tokio::test]
    async fn round_trips_use_fresh_keys_so_late_answers_cant_cross() {
        let mut pending = BridgePending::default();
        let (old_key, old_rx) = pending.wait_for_consent();
        drop(old_rx); // that install timed out and moved on
        pending.consent.remove(&old_key);
        let (new_key, new_rx) = pending.wait_for_consent();
        assert_ne!(old_key, new_key);

        // The stale answer arrives now: it finds nothing to resolve.
        assert!(pending.consent.remove(&old_key).is_none());
        let tx = pending.consent.remove(&new_key).unwrap();
        tx.send(ConsentDecision::JustOnce).unwrap();
        assert_eq!(new_rx.await.unwrap(), ConsentDecision::JustOnce);

        let (a, _) = pending.wait_for_completion();
        let (b, _) = pending.wait_for_completion();
        assert_ne!(a, b);
    }

    #[test]
    fn recent_bridge_files_cover_running_and_just_finished_installs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mod.zip");
        std::fs::write(&file, b"x").unwrap();
        let other = dir.path().join("other.zip");
        std::fs::write(&other, b"x").unwrap();

        let recent = RecentBridgeFiles::default();
        assert!(!recent.contains(&file));
        recent.begin(&file);
        // Spelled differently, still the same file.
        assert!(recent.contains(&dir.path().join(".").join("mod.zip")));
        assert!(!recent.contains(&other));
        recent.finish(&file);
        assert!(recent.contains(&file));

        // Long after it finished: offered normally again.
        recent.0.lock().unwrap().values_mut().for_each(|t| {
            *t = Some(Instant::now() - RecentBridgeFiles::KEEP_AFTER - Duration::from_secs(1))
        });
        assert!(!recent.contains(&file));
    }
}
