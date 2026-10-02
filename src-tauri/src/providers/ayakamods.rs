//! AyakaMods: no public API, but every mod page carries a JSON-LD
//! `SoftwareApplication` block whose `softwareVersion` is the current
//! version and whose `dateModified` is when the mod was last updated.
//! Downloads need a login, so updates go through the browser.
//!
//! The site sits behind Cloudflare, so DDMM is careful to be a polite,
//! honest client (never a disguised browser):
//!
//! - every AyakaMods request, from update checks and installs alike, goes
//!   through one gate: strictly one at a time, at least [`REQUEST_GAP`]
//!   apart;
//! - normal `Accept` / `Accept-Language` headers next to DDMM's own
//!   User-Agent;
//! - the page's canonical address (`/mods/<slug>.<id>/`) is remembered, so
//!   later checks skip the `/mods/<id>/` redirect hop;
//! - a 403/429/503 is retried once after a pause (honouring `Retry-After`);
//!   a Cloudflare challenge is never retried and never "solved": it is
//!   reported as "AyakaMods is blocking automated checks right now".

use std::{collections::HashMap, time::Duration};

use reqwest::{header, StatusCode};

use super::read_capped;

pub const HOST: &str = "ayakamods.com";
const BASE: &str = "https://ayakamods.com";

/// Minimum gap between two AyakaMods requests (any caller).
pub const REQUEST_GAP: Duration = Duration::from_secs(2);
/// Pause before the one retry after a 403/429/503 without `Retry-After`.
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// A `Retry-After` longer than this isn't waited out: the check reports
/// "try again later" instead.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);

const ACCEPT: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8";
const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.8";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AyakaModsMetadata {
    pub name: Option<String>,
    pub latest_version: Option<String>,
    /// `dateModified` as written on the page.
    pub modified: Option<String>,
    /// `dateModified` as Unix seconds: when the mod was last updated.
    pub modified_at: Option<i64>,
    /// The server's clock (its `Date` header, Unix seconds) when the page
    /// was fetched -- lets a check tell how far off this PC's clock is.
    pub server_time: Option<i64>,
}

/// Why an AyakaMods page couldn't be read. The `Display` text is what the
/// user sees after "Couldn't check:".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AyakaError {
    /// 404: the mod is gone, hidden, or the id is wrong.
    NotFound,
    /// A Cloudflare challenge page ("checking your browser").
    Challenge,
    /// 403 without a challenge, still after one retry.
    Forbidden,
    /// 429 / 503, still after one retry (or a `Retry-After` too long to wait).
    Busy { status: u16 },
    /// Any other unsuccessful status.
    Http { status: u16 },
    /// Not a usable mod id.
    BadId(String),
    /// Network, size limit, ...
    Other(String),
}

impl AyakaError {
    /// The HTTP status behind this error, if there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            AyakaError::NotFound => Some(404),
            AyakaError::Challenge | AyakaError::Forbidden => Some(403),
            AyakaError::Busy { status } | AyakaError::Http { status } => Some(*status),
            AyakaError::BadId(_) | AyakaError::Other(_) => None,
        }
    }

    /// Whether the site as a whole is turning DDMM away right now, so the
    /// rest of this check shouldn't ask it again.
    pub fn site_is_refusing(&self) -> bool {
        matches!(self, AyakaError::Challenge | AyakaError::Busy { .. })
    }
}

impl std::fmt::Display for AyakaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AyakaError::NotFound => {
                write!(f, "This mod's page on AyakaMods can't be found (it may have been removed or moved)")
            }
            AyakaError::Challenge => write!(
                f,
                "AyakaMods is blocking automated checks right now. Try again later, or check the mod's page in your browser"
            ),
            AyakaError::Forbidden => write!(
                f,
                "AyakaMods refused the check (403 Forbidden): it may be blocking automated checks right now, or the mod \
                 may only be visible when logged in. Try again later, or check the mod's page in your browser"
            ),
            AyakaError::Busy { status } => write!(
                f,
                "AyakaMods is busy or limiting requests right now ({}). Try again later",
                status_text(*status)
            ),
            AyakaError::Http { status } => write!(f, "ayakamods.com answered {}", status_text(*status)),
            AyakaError::BadId(id) => write!(f, "\"{id}\" isn't an AyakaMods mod id"),
            AyakaError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for AyakaError {}

fn status_text(status: u16) -> String {
    StatusCode::from_u16(status).map(|s| s.to_string()).unwrap_or_else(|_| status.to_string())
}

/// Timings, so tests don't have to wait real seconds.
#[derive(Debug, Clone, Copy)]
struct Timing {
    gap: Duration,
    retry_delay: Duration,
    max_retry_wait: Duration,
}

const TIMING: Timing = Timing { gap: REQUEST_GAP, retry_delay: RETRY_DELAY, max_retry_wait: MAX_RETRY_WAIT };

/// The one gate every AyakaMods request goes through: held for the whole
/// request (retry included), it keeps them strictly sequential; the value
/// is when the last one finished.
static GATE: tokio::sync::Mutex<Option<tokio::time::Instant>> = tokio::sync::Mutex::const_new(None);

/// Canonical page URL per (base, id), learned from the `/mods/<id>/`
/// redirect, for this session.
fn canonical_urls() -> &'static std::sync::Mutex<HashMap<String, String>> {
    static URLS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    URLS.get_or_init(Default::default)
}

/// The numeric AyakaMods id in `id`: a bare id (`4101`) or a page slug
/// with its id (`enhanced-bolt-pistol.4101`).
pub fn normalize_id(id: &str) -> Option<String> {
    let id = id.trim().trim_matches('/');
    let tail = id.rsplit('.').next()?;
    (!tail.is_empty() && tail.len() <= 12 && tail.bytes().all(|b| b.is_ascii_digit())).then(|| tail.to_string())
}

pub(crate) async fn fetch_ayakamods_metadata(
    client: &reqwest::Client,
    id: &str,
) -> Result<Option<AyakaModsMetadata>, AyakaError> {
    #[cfg(test)]
    if let Ok(base) = test_support::BASE.try_with(|b| b.clone()) {
        // Tests elsewhere point AyakaMods at a local (plain http) server.
        let client = reqwest::Client::builder()
            .user_agent(super::user_agent())
            .build()
            .map_err(|e| AyakaError::Other(e.to_string()))?;
        return fetch_from(&client, &base, id, test_support::FAST).await;
    }
    fetch_from(client, BASE, id, TIMING).await
}

/// A scripted local AyakaMods for tests (here and in other modules).
#[cfg(test)]
pub(crate) mod test_support {
    use super::Timing;
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tokio::task_local! {
        /// While set (`BASE.scope(...)`), `fetch_ayakamods_metadata` asks
        /// this base URL instead of the real site.
        pub(crate) static BASE: String;
    }

    pub(super) const FAST: Timing = Timing {
        gap: Duration::from_millis(0),
        retry_delay: Duration::from_millis(10),
        max_retry_wait: Duration::from_secs(5),
    };

    /// Answers each request with the next response (repeating the last),
    /// and records the raw requests.
    pub(crate) async fn server(responses: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let mut i = 0usize;
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                log.lock().unwrap().push(req);
                let resp = responses[i.min(responses.len() - 1)].replace("{addr}", &addr.to_string());
                i += 1;
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    pub(crate) fn reply(status: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// A mod page with this version and `dateModified`.
    pub(crate) fn page(version: &str, date_modified: &str) -> String {
        format!(
            r#"<script type="application/ld+json">[{{"@type":"SoftwareApplication","name":"M","softwareVersion":"{version}","dateModified":"{date_modified}"}}]</script>"#
        )
    }
}

async fn fetch_from(
    client: &reqwest::Client,
    base: &str,
    id: &str,
    timing: Timing,
) -> Result<Option<AyakaModsMetadata>, AyakaError> {
    let id = normalize_id(id).ok_or_else(|| AyakaError::BadId(id.to_string()))?;
    let plain_url = format!("{base}/mods/{id}/");
    let cache_key = format!("{base}#{id}");
    let cached = canonical_urls().lock().ok().and_then(|m| m.get(&cache_key).cloned());

    let mut gate = GATE.lock().await;
    let mut url = cached.clone().unwrap_or_else(|| plain_url.clone());
    let mut retried = false;
    loop {
        if let Some(last) = *gate {
            let since = last.elapsed();
            if since < timing.gap {
                tokio::time::sleep(timing.gap - since).await;
            }
        }
        let result = get_once(client, &url).await;
        *gate = Some(tokio::time::Instant::now());
        match result {
            Ok((html, final_url, server_time)) => {
                if let Some(final_url) = final_url.filter(|u| is_canonical_page(u, base, &id)) {
                    if let Ok(mut m) = canonical_urls().lock() {
                        m.insert(cache_key, final_url);
                    }
                }
                return Ok(parse_ayakamods_page(&html).map(|meta| AyakaModsMetadata { server_time, ..meta }));
            }
            // The remembered address stopped working: forget it and ask
            // the plain one (the site redirects that to wherever it is now).
            Err(Attempt::Status { status: 404, .. }) if url != plain_url => {
                if let Ok(mut m) = canonical_urls().lock() {
                    m.remove(&cache_key);
                }
                url = plain_url.clone();
            }
            Err(Attempt::Status { status: 404, .. }) => return Err(AyakaError::NotFound),
            Err(Attempt::Status { challenge: true, .. }) => return Err(AyakaError::Challenge),
            Err(Attempt::Status { status, retry_after, .. }) if matches!(status, 403 | 429 | 503) => {
                let refused = || if status == 403 { AyakaError::Forbidden } else { AyakaError::Busy { status } };
                if retried {
                    return Err(refused());
                }
                let wait = retry_after.unwrap_or(timing.retry_delay);
                if wait > timing.max_retry_wait {
                    return Err(refused());
                }
                retried = true;
                log::info!("AyakaMods answered {status} for mod {id}; trying once more in {}s.", wait.as_secs());
                tokio::time::sleep(wait).await;
            }
            Err(Attempt::Status { status, .. }) => return Err(AyakaError::Http { status }),
            Err(Attempt::Failed(message)) => return Err(AyakaError::Other(message)),
        }
    }
}

enum Attempt {
    Status { status: u16, retry_after: Option<Duration>, challenge: bool },
    Failed(String),
}

/// One GET: the page body and the address it ended up at (after
/// redirects), or what went wrong.
async fn get_once(client: &reqwest::Client, url: &str) -> Result<(String, Option<String>, Option<i64>), Attempt> {
    let response = client
        .get(url)
        .header(header::ACCEPT, ACCEPT)
        .header(header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
        .send()
        .await
        .map_err(|e| Attempt::Failed(format!("couldn't reach {HOST}: {}", e.without_url())))?;
    let status = response.status();
    let final_url = response.url().to_string();
    if status.is_success() {
        let server_time = response
            .headers()
            .get(header::DATE)
            .and_then(|v| v.to_str().ok())
            .and_then(super::parse_http_date);
        let body = read_capped(response).await.map_err(|e| Attempt::Failed(e.to_string()))?;
        return Ok((body, Some(final_url), server_time));
    }
    let headers = response.headers().clone();
    let mut challenge = is_challenge_header(&headers);
    if !challenge && matches!(status.as_u16(), 403 | 503) {
        // Challenge pages are small; the error page is only read to tell
        // a challenge from a plain refusal.
        if let Ok(body) = read_capped(response).await {
            challenge = is_challenge_page(&body);
        }
    }
    Err(Attempt::Status { status: status.as_u16(), retry_after: retry_after(&headers), challenge })
}

/// `cf-mitigated: challenge` is how Cloudflare marks a challenge response.
fn is_challenge_header(headers: &header::HeaderMap) -> bool {
    headers
        .get("cf-mitigated")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("challenge"))
}

/// A Cloudflare challenge ("Just a moment...") rather than the site's own
/// error page.
pub fn is_challenge_page(html: &str) -> bool {
    html.contains("/cdn-cgi/challenge-platform/") || html.contains("window._cf_chl_opt") || html.contains("cf-chl-")
}

/// `Retry-After` in seconds (the HTTP-date form is rare here and ignored).
fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?;
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Whether `url` is this mod's page on the same site (so it's safe to
/// remember as the address to ask next time).
fn is_canonical_page(url: &str, base: &str, id: &str) -> bool {
    let (Ok(parsed), Ok(base)) = (reqwest::Url::parse(url), reqwest::Url::parse(base)) else { return false };
    parsed.scheme() == base.scheme()
        && parsed.host_str() == base.host_str()
        && parsed.port() == base.port()
        && parsed.query().is_none()
        && parsed.path().starts_with("/mods/")
        && parsed.path().trim_end_matches('/').rsplit(['/', '.']).next() == Some(id)
}

/// Extract the mod's `SoftwareApplication` JSON-LD block from a saved (or
/// freshly-fetched) AyakaMods mod page. `None` for anything that doesn't
/// parse -- missing/malformed JSON-LD is not an error, it's just
/// "unknown" to the caller.
pub fn parse_ayakamods_page(html: &str) -> Option<AyakaModsMetadata> {
    for block in extract_ld_json_blocks(html) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&block) else {
            continue;
        };
        if let Some(app) = find_software_application(&value) {
            let modified = app.get("dateModified").and_then(|v| v.as_str()).map(str::to_string);
            return Some(AyakaModsMetadata {
                name: app.get("name").and_then(|v| v.as_str()).map(str::to_string),
                latest_version: app
                    .get("softwareVersion")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string),
                modified_at: modified.as_deref().and_then(super::parse_iso_utc),
                server_time: None,
                modified,
            });
        }
    }
    None
}

/// Scan `html` for `<script type="application/ld+json">...</script>`
/// blocks. Deliberately simple (no HTML parser crate, just `regex` -- an
/// existing dependency): good enough for extracting a well-formed script
/// tag's contents, not a general HTML parser.
fn extract_ld_json_blocks(html: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(?is)<script[^>]*type\s*=\s*"application/ld\+json"[^>]*>(.*?)</script>"#)
            .unwrap()
    });
    re.captures_iter(html)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Depth-first search through a JSON-LD value (a plain object, an array of
/// objects/graphs, or an object with an `@graph` array) for the first node
/// whose `@type` is `SoftwareApplication`.
fn find_software_application(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => items.iter().find_map(find_software_application),
        serde_json::Value::Object(map) => {
            if map.get("@type").and_then(|v| v.as_str()) == Some("SoftwareApplication") {
                return Some(value);
            }
            map.get("@graph").and_then(find_software_application)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FIXTURE: &str = include_str!("../../tests/fixtures/ayakamods_mod_page.html");
    const BOLT_PISTOL: &str = include_str!("../../tests/fixtures/ayakamods_bolt_pistol_4101.html");

    const FAST: Timing = test_support::FAST;

    #[test]
    fn parses_real_ayakamods_fixture() {
        let meta = parse_ayakamods_page(FIXTURE).expect("should find JSON-LD");
        assert_eq!(meta.name.as_deref(), Some("HD2 Auto Reload"));
        assert_eq!(meta.latest_version.as_deref(), Some("2026-09-24"));
        assert!(meta.modified.is_some());
        assert_eq!(meta.modified_at, super::super::parse_iso_utc("2026-09-24T05:12:01Z"));
    }

    /// The page from issue #59: the version is just the date of the last
    /// update (the site's history lists two "2026-10-01" releases and five
    /// "2026-09-29" ones), so the exact last-update time is what tells
    /// same-day updates apart.
    #[test]
    fn parses_the_bolt_pistol_page() {
        let meta = parse_ayakamods_page(BOLT_PISTOL).expect("should find JSON-LD");
        assert_eq!(meta.name.as_deref(), Some("Enhanced Bolt Pistol Ammunition & Demolition Force"));
        assert_eq!(meta.latest_version.as_deref(), Some("2026-10-01"));
        assert_eq!(meta.modified.as_deref(), Some("2026-10-01T22:10:08+01:00"));
        // The page's own `data-timestamp` for that time.
        assert_eq!(meta.modified_at, Some(1_790_889_008));
    }

    #[test]
    fn missing_ld_json_is_none_not_error() {
        assert!(parse_ayakamods_page("<html><body>no json-ld here</body></html>").is_none());
    }

    #[test]
    fn malformed_ld_json_is_none_not_error() {
        let html = r#"<script type="application/ld+json">{ not: valid json </script>"#;
        assert!(parse_ayakamods_page(html).is_none());
    }

    #[test]
    fn ld_json_without_software_application_is_none() {
        let html = r#"<script type="application/ld+json">[{"@type":"Organization","name":"Someone"}]</script>"#;
        assert!(parse_ayakamods_page(html).is_none());
    }

    #[test]
    fn ld_json_graph_form_is_found() {
        let html = r#"<script type="application/ld+json">
            {"@graph": [{"@type":"Organization"}, {"@type":"SoftwareApplication","name":"Graph Mod","softwareVersion":"1.2.3"}]}
        </script>"#;
        let meta = parse_ayakamods_page(html).expect("should find nested graph entry");
        assert_eq!(meta.name.as_deref(), Some("Graph Mod"));
        assert_eq!(meta.latest_version.as_deref(), Some("1.2.3"));
        assert_eq!(meta.modified_at, None);
    }

    #[test]
    fn blank_version_is_none() {
        let html = r#"<script type="application/ld+json">{"@type":"SoftwareApplication","softwareVersion":"  ","dateModified":"2026-10-01T22:10:08+01:00"}</script>"#;
        let meta = parse_ayakamods_page(html).unwrap();
        assert_eq!(meta.latest_version, None);
        assert_eq!(meta.modified_at, Some(1_790_889_008));
    }

    #[test]
    fn ids_are_numeric() {
        assert_eq!(normalize_id("4101").as_deref(), Some("4101"));
        assert_eq!(normalize_id("enhanced-bolt-pistol.4101").as_deref(), Some("4101"));
        assert_eq!(normalize_id("bastion-maelstrom-360%C2%B0-turret.4284").as_deref(), Some("4284"));
        assert_eq!(normalize_id("hd2-auto-reload"), None);
        assert_eq!(normalize_id(""), None);
        assert_eq!(normalize_id("../4101"), None);
    }

    #[test]
    fn challenge_pages_are_recognized() {
        assert!(is_challenge_page(
            r#"<html><head><title>Just a moment...</title></head><body><script src="/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1"></script></body></html>"#
        ));
        assert!(!is_challenge_page("<html><title>Oops! We ran into some problems.</title></html>"));
        assert!(!is_challenge_page(BOLT_PISTOL));
    }

    #[test]
    fn messages_say_what_happened() {
        assert!(AyakaError::NotFound.to_string().contains("can't be found (it may have been removed or moved)"));
        assert!(AyakaError::Challenge.to_string().contains("blocking automated checks"));
        assert_eq!(AyakaError::Http { status: 500 }.to_string(), "ayakamods.com answered 500 Internal Server Error");
        assert!(AyakaError::Busy { status: 429 }.to_string().contains("429 Too Many Requests"));
        assert!(AyakaError::Challenge.site_is_refusing());
        assert!(!AyakaError::NotFound.site_is_refusing());
    }

    #[test]
    fn only_this_mods_page_is_remembered() {
        let base = "https://ayakamods.com";
        assert!(is_canonical_page("https://ayakamods.com/mods/enhanced-bolt-pistol.4101/", base, "4101"));
        assert!(is_canonical_page("https://ayakamods.com/mods/4101/", base, "4101"));
        assert!(!is_canonical_page("https://ayakamods.com/mods/other.4102/", base, "4101"));
        assert!(!is_canonical_page("https://ayakamods.com/login/?r=mods/x.4101/", base, "4101"));
        assert!(!is_canonical_page("https://evil.example/mods/x.4101/", base, "4101"));
    }

    use test_support::{reply, server};

    fn page(version: &str) -> String {
        test_support::page(version, "2026-10-01T22:10:08+01:00")
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().user_agent(super::super::user_agent()).build().unwrap()
    }

    fn paths(seen: &Mutex<Vec<String>>) -> Vec<String> {
        seen.lock().unwrap().iter().map(|r| r.split_whitespace().nth(1).unwrap_or("").to_string()).collect()
    }

    #[tokio::test]
    async fn sends_honest_polite_headers() {
        let (base, seen) = server(vec![reply("200 OK", "", &page("1.0"))]).await;
        fetch_from(&client(), &base, "1", FAST).await.unwrap();
        let req = seen.lock().unwrap()[0].to_ascii_lowercase();
        assert!(req.contains("user-agent: democracydefendermodmanager/"), "{req}");
        assert!(req.contains("accept: text/html"), "{req}");
        assert!(req.contains("accept-language: en"), "{req}");
    }

    #[tokio::test]
    async fn the_server_clock_is_reported() {
        let (base, _seen) =
            server(vec![reply("200 OK", "Date: Wed, 01 Oct 2026 21:10:08 GMT\r\n", &page("1.0"))]).await;
        let meta = fetch_from(&client(), &base, "8", FAST).await.unwrap().unwrap();
        assert_eq!(meta.server_time, Some(1_790_889_008));
    }

    #[tokio::test]
    async fn not_found_is_reported_plainly() {
        let (base, seen) = server(vec![reply("404 Not Found", "", "<title>404 - Page Not Found</title>")]).await;
        assert_eq!(fetch_from(&client(), &base, "2", FAST).await, Err(AyakaError::NotFound));
        assert_eq!(paths(&seen), ["/mods/2/"], "no retry for a 404");
    }

    #[tokio::test]
    async fn a_403_is_retried_once_then_reported() {
        let (base, seen) = server(vec![reply("403 Forbidden", "", "<title>Oops!</title>")]).await;
        assert_eq!(fetch_from(&client(), &base, "3", FAST).await, Err(AyakaError::Forbidden));
        assert_eq!(paths(&seen).len(), 2);
    }

    #[tokio::test]
    async fn a_passing_403_succeeds_on_the_retry() {
        let (base, seen) =
            server(vec![reply("403 Forbidden", "", "nope"), reply("200 OK", "", &page("2026-10-01"))]).await;
        let meta = fetch_from(&client(), &base, "4", FAST).await.unwrap().unwrap();
        assert_eq!(meta.latest_version.as_deref(), Some("2026-10-01"));
        assert_eq!(paths(&seen).len(), 2);
    }

    #[tokio::test]
    async fn a_challenge_is_never_retried() {
        let (base, seen) = server(vec![reply("403 Forbidden", "cf-mitigated: challenge\r\n", "Just a moment...")]).await;
        assert_eq!(fetch_from(&client(), &base, "5", FAST).await, Err(AyakaError::Challenge));
        let (base2, seen2) = server(vec![reply(
            "403 Forbidden",
            "",
            r#"<script src="/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1"></script>"#,
        )])
        .await;
        assert_eq!(fetch_from(&client(), &base2, "5", FAST).await, Err(AyakaError::Challenge));
        assert_eq!(paths(&seen).len(), 1);
        assert_eq!(paths(&seen2).len(), 1);
    }

    #[tokio::test]
    async fn retry_after_is_respected_and_a_long_one_isnt_waited_out() {
        let (base, seen) =
            server(vec![reply("429 Too Many Requests", "Retry-After: 0\r\n", ""), reply("200 OK", "", &page("1.1"))]).await;
        assert!(fetch_from(&client(), &base, "6", FAST).await.unwrap().is_some());
        assert_eq!(paths(&seen).len(), 2);

        let (base, seen) = server(vec![reply("503 Service Unavailable", "Retry-After: 3600\r\n", "")]).await;
        assert_eq!(fetch_from(&client(), &base, "6", FAST).await, Err(AyakaError::Busy { status: 503 }));
        assert_eq!(paths(&seen).len(), 1, "an hour is not waited out");
    }

    #[tokio::test]
    async fn the_canonical_page_is_remembered_after_the_redirect() {
        let (base, seen) = server(vec![
            reply("301 Moved Permanently", "Location: http://{addr}/mods/cool-mod.7/\r\n", ""),
            reply("200 OK", "", &page("1.0")),
        ])
        .await;
        fetch_from(&client(), &base, "7", FAST).await.unwrap();
        fetch_from(&client(), &base, "7", FAST).await.unwrap();
        assert_eq!(paths(&seen), ["/mods/7/", "/mods/cool-mod.7/", "/mods/cool-mod.7/"]);
    }

    #[tokio::test]
    async fn bad_ids_make_no_request() {
        let (base, seen) = server(vec![reply("200 OK", "", &page("1.0"))]).await;
        assert!(matches!(fetch_from(&client(), &base, "not-an-id", FAST).await, Err(AyakaError::BadId(_))));
        assert!(paths(&seen).is_empty());
    }

    /// Real network smoke test against the live page. `#[ignore]`d so the
    /// normal suite never depends on network access.
    #[tokio::test]
    #[ignore]
    async fn fetches_and_parses_the_real_page() {
        let client = super::super::build_client().unwrap();
        let meta = fetch_ayakamods_metadata(&client, "4101")
            .await
            .unwrap()
            .expect("should parse JSON-LD from the live page");
        assert!(meta.latest_version.is_some());
        assert!(meta.modified_at.is_some());
        // A unicode slug behind the redirect.
        let meta = fetch_ayakamods_metadata(&client, "4284").await.unwrap();
        assert!(meta.is_some());
    }
}
