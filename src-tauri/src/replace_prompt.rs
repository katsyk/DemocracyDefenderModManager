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

use std::{collections::HashMap, future::Future, pin::Pin, time::Duration};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
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
/// The longest a question about a browser install waits.
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(120);
/// The extension gives up on an install five minutes after sending it
/// (`INSTALL_TIMEOUT_MS` in its protocol client): waiting for the Mods page,
/// the consent prompt and the install queue included.
const EXTENSION_INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Left for the install itself and the afterInstall step once the question
/// is answered.
const BROWSER_MARGIN: Duration = Duration::from_secs(120);
/// Less time than this for the question: don't ask at all.
const MIN_QUESTION_TIME: Duration = Duration::from_secs(20);

/// How long the question about a browser install that arrived `elapsed`
/// ago may wait, so that a Replace never happens after the extension has
/// given up (and shown a failure); `None` when there's too little time left
/// to ask.
pub fn browser_question_timeout(elapsed: Duration) -> Option<Duration> {
    let left = EXTENSION_INSTALL_TIMEOUT.saturating_sub(elapsed).saturating_sub(BROWSER_MARGIN);
    (left >= MIN_QUESTION_TIME).then(|| left.min(BROWSER_TIMEOUT))
}

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
    User { app: &'a dyn AskUser, timeout: Duration },
    /// A fixed answer (tests).
    #[cfg(test)]
    Answer(bool),
}

impl<'a> Asker<'a> {
    pub fn in_app<R: Runtime>(app: &'a AppHandle<R>) -> Self {
        Asker::User { app, timeout: IN_APP_TIMEOUT }
    }

    /// For a browser install that arrived at `arrived`: asks only while
    /// the extension still waits for the answer (see
    /// [`browser_question_timeout`]); otherwise nobody is asked and the
    /// file is refused as already installed.
    pub fn browser<R: Runtime>(app: &'a AppHandle<R>, arrived: std::time::Instant) -> Self {
        match browser_question_timeout(arrived.elapsed()) {
            Some(timeout) => Asker::User { app, timeout },
            None => {
                log::warn!("Too little time left before the browser extension gives up; not asking whether to replace.");
                Asker::Never
            }
        }
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
            Asker::User { app, timeout } => app.ask_user(offer, *timeout).await,
            #[cfg(test)]
            Asker::Answer(answer) => Some(*answer),
        }
    }
}

/// Asks the user on the Mods page. Behind a trait object so that only the
/// commands (which build an [`Asker::User`] from their `AppHandle`) reach
/// the real webview runtime: code the unit tests run (with
/// [`Asker::Answer`]) never references it, and so the test binary doesn't
/// link the window/webview code (on Windows it would then need the app's
/// common-controls manifest just to start).
pub trait AskUser: Sync {
    fn ask_user<'a>(
        &'a self,
        offer: &'a ReplaceOffer,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Option<bool>> + Send + 'a>>;
}

impl<R: Runtime> AskUser for AppHandle<R> {
    fn ask_user<'a>(
        &'a self,
        offer: &'a ReplaceOffer,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Option<bool>> + Send + 'a>> {
        Box::pin(ask_user(self, offer, timeout))
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

    /// The Mods page went away (reloaded, left): nobody will answer the
    /// open questions, so each is answered "keep" at once -- rather than
    /// holding its install (and the data folder) until it times out.
    pub fn keep_all(&mut self) -> usize {
        let n = self.0.len();
        for (_, tx) in self.0.drain() {
            let _ = tx.send(false);
        }
        n
    }
}

async fn ask_user<R: Runtime>(app: &AppHandle<R>, offer: &ReplaceOffer, timeout: Duration) -> Option<bool> {
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

    #[cfg(not(windows))]
    fn offer() -> ReplaceOffer {
        ReplaceOffer {
            guid: Uuid::nil(),
            file_name: "Cool Mod.zip".into(),
            installed_name: "Cool Mod".into(),
            installed_version: None,
            new_name: "Cool Mod".into(),
            new_version: None,
            same_name: true,
        }
    }

    /// The real ask path, on Tauri's mock runtime. Not on Windows: the
    /// mock runtime isn't available there (see the `tauri` dev-dependency
    /// in Cargo.toml).
    #[cfg(not(windows))]
    mod ask_path {
        use super::*;

        /// A mock app (no webview) with DDMM's state, the Mods page ready, and
        /// the request IDs of the questions it was asked / told were closed.
        fn mock_page() -> (
            tauri::App<tauri::test::MockRuntime>,
            tempfile::TempDir,
            std::sync::Arc<std::sync::Mutex<Vec<String>>>,
            std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        ) {
            use tauri::Listener;
            let app = tauri::test::mock_app();
            let dir = tempfile::tempdir().unwrap();
            app.manage(AppState::new(dir.path().to_path_buf()));
            app.state::<AppState>().bridge_frontend_ready.send_replace(true);
            let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let closed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            for (event, list) in [(REQUEST_EVENT, asked.clone()), (CLOSED_EVENT, closed.clone())] {
                app.listen_any(event, move |e| {
                    let v: serde_json::Value = serde_json::from_str(e.payload()).unwrap();
                    list.lock().unwrap().push(v["requestId"].as_str().unwrap().to_string());
                });
            }
            (app, dir, asked, closed)
        }

        async fn wait_for_question(asked: &std::sync::Mutex<Vec<String>>) -> String {
            for _ in 0..200 {
                if let Some(id) = asked.lock().unwrap().first().cloned() {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("the question was never asked");
        }

        #[tokio::test]
        async fn the_page_answers_through_resolve() {
            let (app, _dir, asked, _) = mock_page();
            let handle = app.handle().clone();
            let question = tokio::spawn(async move { Asker::in_app(&handle).ask(&offer()).await });
            let id = wait_for_question(&asked).await;
            assert!(app.state::<AppState>().replace_pending.lock().await.answer(&id, true));
            assert_eq!(question.await.unwrap(), Some(true));
        }

        /// No answer in time: keep, and the page is told to close the question.
        #[tokio::test]
        async fn no_answer_in_time_keeps_and_closes_the_question() {
            let (app, _dir, asked, closed) = mock_page();
            let handle = app.handle().clone();
            let answer = Asker::User { app: &handle, timeout: Duration::from_millis(50) }.ask(&offer()).await;
            assert_eq!(answer, Some(false));
            assert_eq!(*closed.lock().unwrap(), *asked.lock().unwrap());
            assert_eq!(closed.lock().unwrap().len(), 1);
            assert_eq!(app.state::<AppState>().replace_pending.lock().await.0.len(), 0, "nothing left waiting");
        }

        /// The page went away while asking: keep, at once.
        #[tokio::test]
        async fn the_page_going_away_keeps_at_once() {
            let (app, _dir, asked, _) = mock_page();
            let handle = app.handle().clone();
            let question = tokio::spawn(async move { Asker::in_app(&handle).ask(&offer()).await });
            wait_for_question(&asked).await;
            assert_eq!(app.state::<AppState>().replace_pending.lock().await.keep_all(), 1);
            let answer = tokio::time::timeout(Duration::from_secs(5), question).await.unwrap().unwrap();
            assert_eq!(answer, Some(false));
        }

    }

    #[test]
    fn a_browser_question_never_outlasts_the_extensions_wait() {
        assert_eq!(browser_question_timeout(Duration::ZERO), Some(BROWSER_TIMEOUT));
        let late = browser_question_timeout(Duration::from_secs(150)).unwrap();
        assert!(Duration::from_secs(150) + late + BROWSER_MARGIN <= EXTENSION_INSTALL_TIMEOUT);
        assert_eq!(browser_question_timeout(Duration::from_secs(170)), None, "too little time left to ask");
        assert_eq!(browser_question_timeout(Duration::from_secs(600)), None);
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
