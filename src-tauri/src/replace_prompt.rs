//! "Replace the installed mod with this file?" -- asked when a file being
//! added carries, in its manifest.json, the ID (GUID) of a mod that's
//! already installed.
//!
//! Every way an archive is added (Add, drag and drop, Add URL, a browser
//! handoff, the browser extension) asks the same question on the Mods
//! page: the backend emits [`REQUEST_EVENT`] with a [`ReplaceOffer`] and
//! waits, without holding the mod list, for the page to answer through
//! `resolve_replace_request`. Replace swaps the file in for the installed
//! mod in place (see `commands::mods::replace_from_archive`); Cancel, no
//! answer, or a page that can't ask leave everything as it is.

use std::{collections::HashMap, time::Duration};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::AppState;

/// Asks the Mods page to show the question (payload: `requestId`, `offer`).
pub const REQUEST_EVENT: &str = "install://replace-request";
/// Tells the Mods page a question is no longer waited for (timed out), so
/// it closes it (payload: `requestId`).
pub const CLOSED_EVENT: &str = "install://replace-request-closed";

/// How long a question about a file added in the app waits for an answer.
pub const IN_APP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long a question about a browser install waits: the extension gives
/// up on an install after five minutes, consent prompt included.
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(120);

/// What the question shows: the installed mod and the file that has its ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceOffer {
    /// The ID both share.
    pub guid: Uuid,
    /// The file being added (its name only).
    pub file_name: String,
    pub installed_name: String,
    /// What DDMM knows of the installed mod's version (from where it was
    /// installed from), if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// The name in the file's manifest.json.
    pub new_name: String,
    /// The file's version, when its name or manifest says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    /// Whether both have the same name: most likely another version of the
    /// same mod. When not, they may be two different mods whose author
    /// copied another mod's manifest.json.
    pub same_name: bool,
}

/// Who answers "replace?" for an install.
pub enum Asker<'a> {
    /// Nobody: a file with an installed mod's ID is refused as already
    /// installed (the bulk import, which has its own statuses).
    Never,
    /// The user, on the Mods page.
    User { app: &'a AppHandle, timeout: Duration },
    /// A fixed answer (tests).
    #[cfg(test)]
    Answer(bool),
}

impl<'a> Asker<'a> {
    pub fn in_app(app: &'a AppHandle) -> Self {
        Asker::User { app, timeout: IN_APP_TIMEOUT }
    }

    pub fn browser(app: &'a AppHandle) -> Self {
        Asker::User { app, timeout: BROWSER_TIMEOUT }
    }

    pub fn asks(&self) -> bool {
        !matches!(self, Asker::Never)
    }

    /// `Some(true)`: replace. `Some(false)`: keep the installed mod (Cancel,
    /// or no answer in time). `None`: the question couldn't be asked (the
    /// Mods page isn't open); the install then goes on as before and is
    /// refused as already installed.
    pub async fn ask(&self, offer: &ReplaceOffer) -> Option<bool> {
        match self {
            Asker::Never => None,
            Asker::User { app, timeout } => ask_user(app, offer, *timeout).await,
            #[cfg(test)]
            Asker::Answer(answer) => Some(*answer),
        }
    }
}

/// Questions waiting for the Mods page's answer, by a fresh key per
/// question (never anything a caller chose, so a late answer can't
/// resolve another question).
#[derive(Default)]
pub struct PendingReplaces(HashMap<String, oneshot::Sender<bool>>);

impl PendingReplaces {
    fn wait(&mut self) -> (String, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let key = Uuid::new_v4().to_string();
        self.0.insert(key.clone(), tx);
        (key, rx)
    }

    /// Hand the page's answer to the install waiting for it. `false` when
    /// nothing waits for `key` any more (timed out, or answered already).
    pub fn answer(&mut self, key: &str, replace: bool) -> bool {
        match self.0.remove(key) {
            Some(tx) => tx.send(replace).is_ok(),
            None => false,
        }
    }

    fn forget(&mut self, key: &str) {
        self.0.remove(key);
    }
}

async fn ask_user(app: &AppHandle, offer: &ReplaceOffer, timeout: Duration) -> Option<bool> {
    if !crate::bridge::server::wait_for_frontend(app).await {
        log::warn!("The Mods page isn't open to ask whether to replace \"{}\"; not replacing it.", offer.installed_name);
        return None;
    }
    let state = app.state::<AppState>();
    let (key, rx) = state.replace_pending.lock().await.wait();
    crate::bridge::server::focus_main_window(app);
    log::info!(
        "\"{}\" has the ID of the installed \"{}\"; asking whether to replace it.",
        offer.file_name,
        offer.installed_name
    );
    if let Err(e) = app.emit(REQUEST_EVENT, serde_json::json!({ "requestId": key, "offer": offer })) {
        log::error!("Couldn't ask whether to replace \"{}\": {e}", offer.installed_name);
        state.replace_pending.lock().await.forget(&key);
        return None;
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(replace)) => {
            log::info!("{} \"{}\".", if replace { "Replacing" } else { "Keeping" }, offer.installed_name);
            Some(replace)
        }
        // The sender is only dropped unanswered when the question is
        // forgotten, which is a "no".
        Ok(Err(_)) => Some(false),
        Err(_) => {
            state.replace_pending.lock().await.forget(&key);
            let _ = app.emit(CLOSED_EVENT, serde_json::json!({ "requestId": key }));
            log::info!("No answer in time; keeping \"{}\".", offer.installed_name);
            Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_answer_reaches_its_own_question_only_once() {
        let mut pending = PendingReplaces::default();
        let (a, rx_a) = pending.wait();
        let (b, rx_b) = pending.wait();
        assert_ne!(a, b);
        assert!(pending.answer(&b, true));
        assert!(!pending.answer(&b, false), "a second answer goes nowhere");
        assert!(!pending.answer("not-a-question", true));
        assert!(rx_b.await.unwrap());
        pending.forget(&a);
        assert!(rx_a.await.is_err(), "a forgotten question gets no answer");
    }

    #[test]
    fn the_offer_is_sent_in_the_shape_the_page_reads() {
        let offer = ReplaceOffer {
            guid: Uuid::nil(),
            file_name: "Cool Mod-1-2-0.zip".into(),
            installed_name: "Cool Mod".into(),
            installed_version: None,
            new_name: "Cool Mod".into(),
            new_version: Some("2.0".into()),
            same_name: true,
        };
        let value = serde_json::to_value(&offer).unwrap();
        assert_eq!(value["installedName"], "Cool Mod");
        assert_eq!(value["newVersion"], "2.0");
        assert_eq!(value["sameName"], true);
        assert!(value.get("installedVersion").is_none());
    }
}
