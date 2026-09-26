//! `ddmm://` links: any web page (a mod site, an author's page, a Discord
//! message) can offer one-click install without the browser extension.
//! See `docs/development/bridge-protocol.md`.
//!
//! Unlike the bridge, this never installs anything itself -- it only
//! validates the link and queues it for the frontend, which always shows a
//! confirmation (no "always allow", per spec) before running the same Add
//! URL flow a manual paste would. Queued rather than sent, because a link
//! that starts DDMM arrives before the page exists (see [`DeepLinkQueue`]).

use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

use crate::AppState;

/// What a `ddmm://` URL asked for, after validation. Only `https` install
/// targets are accepted; everything else -- a non-`ddmm` scheme, an
/// unrecognized host, a missing/malformed `url` parameter, or a non-`https`
/// target -- is silently ignored, per spec ("Only `https` URLs are
/// accepted. Anything else in the link is ignored.").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkAction {
    Install { url: String },
    Open,
}

pub fn parse(url: &Url) -> Option<DeepLinkAction> {
    if url.scheme() != "ddmm" {
        return None;
    }

    match url.host_str()? {
        "open" => Some(DeepLinkAction::Open),
        "install" => {
            let target = url.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned())?;
            let parsed = Url::parse(&target).ok()?;
            if parsed.scheme() != "https" {
                return None;
            }
            Some(DeepLinkAction::Install { url: target })
        }
        _ => None,
    }
}

/// `ddmm://install` targets waiting for the Mods page to show their
/// confirmation.
///
/// A link can arrive before the page can show anything: on a cold start
/// it's in argv and is read while the app is still setting up, long before
/// the webview has loaded (an event emitted then reaches no one), and the
/// page may be elsewhere (Settings, the data-folder recovery screen) when
/// DDMM is already running. So links are only queued here, and the page
/// takes them with [`take_pending_deep_links`] once it's ready, and again
/// each time [`PENDING_EVENT`] says there's something new. Taking empties
/// the queue, so each link is handed out exactly once however often the
/// page asks. (The page itself skips a link whose confirmation or install
/// is still in progress, so one click delivered twice isn't shown twice.)
#[derive(Debug, Default)]
pub struct DeepLinkQueue {
    pending: Vec<String>,
}

/// At most this many install links wait at once. They pile up only while
/// the Mods page isn't shown (Settings, the recovery screen); anything
/// beyond this is dropped rather than growing without bound.
pub const MAX_PENDING_LINKS: usize = 16;

/// Install targets longer than this (in bytes) are refused. Real mod pages
/// and downloads are far shorter; this keeps a queued link, and the
/// relaunch arguments built from it, reasonably small.
pub const MAX_TARGET_LEN: usize = 2048;

/// What [`DeepLinkQueue::push`] did with a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Queued,
    /// The same target is already waiting (e.g. a cold-start link seen both
    /// in argv and through the deep-link plugin).
    Duplicate,
    /// [`MAX_PENDING_LINKS`] are already waiting; this one was dropped.
    QueueFull,
    /// Longer than [`MAX_TARGET_LEN`]; refused.
    TooLong,
}

impl DeepLinkQueue {
    /// Queue an install target, unless it's too long, already waiting, or
    /// the queue is full.
    pub fn push(&mut self, target: String) -> PushOutcome {
        if target.len() > MAX_TARGET_LEN {
            return PushOutcome::TooLong;
        }
        if self.pending.contains(&target) {
            return PushOutcome::Duplicate;
        }
        if self.pending.len() >= MAX_PENDING_LINKS {
            return PushOutcome::QueueFull;
        }
        self.pending.push(target);
        PushOutcome::Queued
    }

    /// Everything waiting, oldest first, which is then no longer waiting.
    pub fn take_all(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending)
    }

    /// Links still waiting, as `ddmm://install` links again -- for the
    /// relaunch after a data folder change, so a link that arrived while
    /// the recovery screen was up isn't lost. Links already handed to the
    /// page are never in here, so none is shown twice.
    pub fn pending_links(&self) -> Vec<String> {
        self.pending.iter().map(|target| install_link(target)).collect()
    }
}

/// The `ddmm://install` link for an https target (the inverse of [`parse`]).
pub fn install_link(target: &str) -> String {
    let mut link = Url::parse("ddmm://install").expect("static URL");
    link.query_pairs_mut().append_pair("url", target);
    link.to_string()
}

/// `ddmm://` URLs among this process's arguments (the program name
/// excluded). On Windows and Linux a link that starts DDMM arrives as its
/// only argument; the relaunch after a data folder change may pass several.
/// Other arguments are left alone.
pub fn links_from_args<S: AsRef<str>>(args: impl IntoIterator<Item = S>) -> Vec<Url> {
    args.into_iter()
        .skip(1)
        .filter_map(|a| Url::parse(a.as_ref()).ok())
        .filter(|u| u.scheme() == "ddmm")
        .collect()
}

/// Emitted when an install link was queued. The Mods page answers with
/// [`take_pending_deep_links`]; if it isn't listening yet (a cold start,
/// or another page or the recovery screen is shown), it takes them when it
/// mounts.
pub const PENDING_EVENT: &str = "deep-link://pending";

/// Handle every URL from one `on_open_url` event, or the ones DDMM was
/// started with: bring the window to front, and queue each `install` link
/// for the Mods page's confirmation (see [`DeepLinkQueue`]). Never installs
/// anything itself.
pub fn handle(app: &AppHandle, urls: Vec<Url>) {
    let mut queued = false;
    for url in urls {
        match parse(&url) {
            Some(DeepLinkAction::Open) => {
                crate::bridge::server::focus_main_window(app);
            }
            Some(DeepLinkAction::Install { url: target }) => {
                crate::bridge::server::focus_main_window(app);
                let state = app.state::<AppState>();
                let mut queue = state.deep_links.lock().unwrap_or_else(|e| e.into_inner());
                let target_len = target.len();
                match queue.push(target) {
                    PushOutcome::Queued => {
                        log::info!("Install link queued for confirmation: {url}");
                        queued = true;
                    }
                    PushOutcome::Duplicate => {
                        log::info!("Ignoring a repeat of an install link already waiting: {url}");
                    }
                    PushOutcome::QueueFull => {
                        log::warn!(
                            "Ignoring an install link: {MAX_PENDING_LINKS} are already waiting to be shown: {url}"
                        );
                    }
                    PushOutcome::TooLong => {
                        // Not logged in full: it's over the limit by definition.
                        log::warn!(
                            "Ignoring an install link whose target is {target_len} characters long \
                             (the limit is {MAX_TARGET_LEN})."
                        );
                    }
                }
            }
            None => {
                log::debug!("Ignoring unrecognized or non-https deep link: {url}");
            }
        }
    }
    if queued {
        let _ = app.emit(PENDING_EVENT, ());
    }
}

/// Hand the Mods page every queued install link (https targets, oldest
/// first) and forget them. The page calls this once its profiles are
/// loaded and its listeners registered, and on each [`PENDING_EVENT`].
#[tauri::command]
pub fn take_pending_deep_links(state: State<'_, AppState>) -> Vec<String> {
    let taken = state.deep_links.lock().unwrap_or_else(|e| e.into_inner()).take_all();
    if !taken.is_empty() {
        log::info!("Showing {} install link(s) on the Mods page", taken.len());
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn parses_install_with_https_target() {
        let action = parse(&url("ddmm://install?url=https%3A%2F%2Fayakamods.com%2Fmods%2Fcool.4084%2F")).unwrap();
        assert_eq!(action, DeepLinkAction::Install { url: "https://ayakamods.com/mods/cool.4084/".to_string() });
    }

    #[test]
    fn parses_open() {
        assert_eq!(parse(&url("ddmm://open")).unwrap(), DeepLinkAction::Open);
    }

    #[test]
    fn rejects_non_https_install_target() {
        assert!(parse(&url("ddmm://install?url=http%3A%2F%2Fexample.com%2Fmod.zip")).is_none());
        assert!(parse(&url("ddmm://install?url=ftp%3A%2F%2Fexample.com%2Fmod.zip")).is_none());
        assert!(parse(&url("ddmm://install?url=javascript%3Aalert(1)")).is_none());
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(parse(&url("https://install?url=https://example.com")).is_none());
    }

    #[test]
    fn rejects_unrecognized_host() {
        assert!(parse(&url("ddmm://uninstall?url=https://example.com")).is_none());
    }

    #[test]
    fn rejects_install_with_missing_url_param() {
        assert!(parse(&url("ddmm://install")).is_none());
        assert!(parse(&url("ddmm://install?foo=bar")).is_none());
    }

    #[test]
    fn rejects_install_with_malformed_url_param() {
        assert!(parse(&url("ddmm://install?url=not-a-url")).is_none());
    }

    const A: &str = "https://example.com/mods/a.zip";
    const B: &str = "https://example.org/b.zip";

    #[test]
    fn queued_links_wait_until_taken_then_come_out_once_in_order() {
        let mut q = DeepLinkQueue::default();
        // A cold start: links arrive before anything can take them.
        assert_eq!(q.push(A.into()), PushOutcome::Queued);
        assert_eq!(q.push(B.into()), PushOutcome::Queued);
        // The page becomes ready and takes them, oldest first.
        assert_eq!(q.take_all(), vec![A.to_string(), B.to_string()]);
        // Asking again (a second `pending` event, the page remounting)
        // yields nothing: each link is handed out once.
        assert!(q.take_all().is_empty());
        assert!(q.pending_links().is_empty());
    }

    #[test]
    fn the_same_link_waiting_twice_is_queued_once() {
        let mut q = DeepLinkQueue::default();
        // E.g. the cold-start link seen both in argv and via get_current().
        assert_eq!(q.push(A.into()), PushOutcome::Queued);
        assert_eq!(q.push(A.into()), PushOutcome::Duplicate);
        assert_eq!(q.push(B.into()), PushOutcome::Queued);
        assert_eq!(q.push(B.into()), PushOutcome::Duplicate);
        assert_eq!(q.take_all(), vec![A.to_string(), B.to_string()]);
    }

    #[test]
    fn links_arriving_after_a_take_are_queued_for_the_next_one() {
        let mut q = DeepLinkQueue::default();
        assert_eq!(q.push(A.into()), PushOutcome::Queued);
        assert_eq!(q.take_all(), vec![A.to_string()]);
        // Clicking it again later is a new request (the page, not the
        // queue, skips it while its first prompt is still open).
        assert_eq!(q.push(A.into()), PushOutcome::Queued);
        assert_eq!(q.push(B.into()), PushOutcome::Queued);
        assert_eq!(q.take_all(), vec![A.to_string(), B.to_string()]);
    }

    #[test]
    fn links_still_waiting_survive_a_relaunch_as_ddmm_links() {
        let mut q = DeepLinkQueue::default();
        let odd = "https://example.com/dl?id=4&name=a b&x=%2F";
        assert_eq!(q.push(odd.into()), PushOutcome::Queued);
        let links = q.pending_links();
        assert_eq!(links.len(), 1);
        // The relaunched DDMM reads them back to exactly the same target.
        let back = links_from_args(["ddmm".to_string(), links[0].clone()]);
        assert_eq!(parse(&back[0]), Some(DeepLinkAction::Install { url: odd.to_string() }));
        // Once handed to the page, it isn't carried over any more.
        q.take_all();
        assert!(q.pending_links().is_empty());
    }

    #[test]
    fn the_queue_is_capped_and_drops_new_links_when_full() {
        let mut q = DeepLinkQueue::default();
        let target = |i: usize| format!("https://example.com/mods/{i}.zip");
        for i in 0..MAX_PENDING_LINKS {
            assert_eq!(q.push(target(i)), PushOutcome::Queued, "link {i}");
        }
        // Full: the newest is dropped, the ones already waiting are kept.
        assert_eq!(q.push(target(MAX_PENDING_LINKS)), PushOutcome::QueueFull);
        assert_eq!(q.push(target(MAX_PENDING_LINKS + 1)), PushOutcome::QueueFull);
        // A repeat of a waiting link is still reported as a repeat.
        assert_eq!(q.push(target(0)), PushOutcome::Duplicate);
        let taken = q.take_all();
        assert_eq!(taken, (0..MAX_PENDING_LINKS).map(target).collect::<Vec<_>>());
        // Once taken, there's room again.
        assert_eq!(q.push(target(MAX_PENDING_LINKS)), PushOutcome::Queued);
    }

    #[test]
    fn overlong_targets_are_refused() {
        let mut q = DeepLinkQueue::default();
        let base = "https://example.com/";
        let at_limit = format!("{base}{}", "a".repeat(MAX_TARGET_LEN - base.len()));
        let over = format!("{at_limit}a");
        assert_eq!(at_limit.len(), MAX_TARGET_LEN);
        assert_eq!(q.push(over), PushOutcome::TooLong);
        assert!(q.pending_links().is_empty(), "a refused link is never queued");
        assert_eq!(q.push(at_limit.clone()), PushOutcome::Queued);
        assert_eq!(q.take_all(), vec![at_limit]);
    }

    #[test]
    fn links_from_args_takes_only_ddmm_urls_after_the_program_name() {
        let found = links_from_args([
            "ddmm://install?url=https%3A%2F%2Fexample.com%2Fa.zip", // argv[0] is never a link
            "--flag",
            "ddmm://install?url=https%3A%2F%2Fexample.com%2Fmod.zip",
            "https://example.com/not-a-ddmm-link",
            "/home/u/file.zip",
            "ddmm://open",
        ]);
        let found: Vec<String> = found.iter().map(Url::to_string).collect();
        assert_eq!(found, vec!["ddmm://install?url=https%3A%2F%2Fexample.com%2Fmod.zip", "ddmm://open"]);
        assert!(links_from_args(["ddmm"]).is_empty());
        // Native-messaging host arguments are not links.
        assert!(links_from_args(["ddmm", "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/", "--parent-window=0"]).is_empty());
    }
}
