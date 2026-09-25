//! `ddmm://` links: any web page (a mod site, an author's page, a Discord
//! message) can offer one-click install without the browser extension.
//! See `docs/development/bridge-protocol.md`.
//!
//! Unlike the bridge, this never installs anything itself -- it only
//! validates the link and hands it to the frontend, which always shows a
//! confirmation (no "always allow", per spec) before running the same Add
//! URL flow a manual paste would.

use tauri::{AppHandle, Emitter};
use url::Url;

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

/// Handle every URL from one `on_open_url` event (or the initial
/// `get_current()` check at cold start): focus the window, and for an
/// `install` link, ask the frontend to show the confirmation and run the
/// Add URL flow.
pub fn handle(app: &AppHandle, urls: Vec<Url>) {
    for url in urls {
        match parse(&url) {
            Some(DeepLinkAction::Open) => {
                crate::bridge::server::focus_main_window(app);
            }
            Some(DeepLinkAction::Install { url }) => {
                crate::bridge::server::focus_main_window(app);
                let _ = app.emit("deep-link://install-request", serde_json::json!({ "url": url }));
            }
            None => {
                log::debug!("Ignoring unrecognized or non-https deep link: {url}");
            }
        }
    }
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
}
