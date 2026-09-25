//! Optional "Sign in to Nexus Mods": OAuth 2.0 authorization code flow with
//! PKCE (S256), the way Nexus Mods asks desktop apps to integrate
//! (<https://modding.wiki/en/api/oauth2-guide>). Mod Organizer 2's
//! `nexusoauthconfig.cpp` / `nxmaccessmanager.cpp` were the reference for
//! the shape of it (loopback redirect, S256, refresh before expiry, Bearer
//! header plus `Application-Name`/`Application-Version`).
//!
//! The flow:
//! 1. Generate a PKCE verifier/challenge and a random `state`.
//! 2. Listen on `127.0.0.1:28647` only (fixed: the redirect URI is
//!    registered with Nexus, so no other port would be accepted).
//! 3. Open the system browser at the authorize URL.
//! 4. Wait (at most 5 minutes) for `GET /callback?code=...&state=...`,
//!    check `state`, show a "you can close this tab" page, and close the
//!    listener.
//! 5. Exchange the code (with the verifier) at the token endpoint, read the
//!    account name from the userinfo endpoint, and store the tokens in the
//!    OS keychain (`crate::secrets::OAUTH_SLOT`).
//!
//! Afterwards [`resolve_auth`] is the single place that decides how a
//! Nexus API request authenticates: signed in (refreshing the access token
//! shortly before it expires) > personal API key > nothing.
//!
//! Tokens and the authorization code are never logged, never put in error
//! text (see [`SignInError`], which only carries our own wording and
//! Nexus's error *codes*), never reach the frontend or the browser bridge,
//! and are only ever sent to `users.nexusmods.com` (sign-in, refresh,
//! revoke, userinfo) and `api.nexusmods.com` (update checks, see
//! `providers::nexus`).
//!
//! DDMM needs a client ID that Nexus Mods issues on registration (by email).
//! It's compiled in from `DDMM_NEXUS_CLIENT_ID` at build time; the same
//! variable at runtime overrides it (for testing). With neither, sign-in
//! is unavailable ("Coming soon") and the manual API key still works.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::SysRng, TryRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use crate::providers::nexus::NexusAuth;
use crate::providers::{APP_NAME, CONNECT_TIMEOUT, REQUEST_TIMEOUT};
use crate::secrets::{self, KeyStorage, Secret, OAUTH_SLOT};

/// Build-time/run-time variable holding DDMM's Nexus OAuth client ID.
pub const CLIENT_ID_ENV: &str = "DDMM_NEXUS_CLIENT_ID";
/// Compiled-in client ID (empty until Nexus Mods issues one).
const BUILD_CLIENT_ID: &str = match option_env!("DDMM_NEXUS_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// Fixed loopback port for the redirect. Registered with Nexus as part of
/// the redirect URI, so it can't change or fall back to another port.
/// (Deliberately not 28635, which another mod manager uses.)
pub const REDIRECT_PORT: u16 = 28647;
pub const CALLBACK_PATH: &str = "/callback";
/// `openid` for the userinfo endpoint (account name), `public` for the
/// public API. Never `mod_file:quarantine`.
pub const SCOPES: &str = "openid public";
/// How long to wait for the user to finish in the browser.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Refresh the access token when it has less than this left (the same
/// margin Mod Organizer 2 uses).
pub const REFRESH_SKEW_SECS: i64 = 5 * 60;
/// Used when the token response has no `expires_in` and the token's own
/// `exp` can't be read.
const DEFAULT_LIFETIME_SECS: i64 = 60 * 60;

/// Nexus Mods' OAuth endpoints, from
/// <https://users.nexusmods.com/.well-known/openid-configuration>.
/// Hard-coded rather than discovered at runtime: they're stable, all on
/// the one host we allow tokens to go to, and fetching the discovery
/// document would add a request (and a way to be pointed elsewhere) for no
/// benefit.
pub const USERS_HOST: &str = "users.nexusmods.com";
pub const AUTHORIZE_URL: &str = "https://users.nexusmods.com/oauth/authorize";
pub const TOKEN_URL: &str = "https://users.nexusmods.com/oauth/token";
pub const REVOKE_URL: &str = "https://users.nexusmods.com/oauth/revoke";
pub const USERINFO_URL: &str = "https://users.nexusmods.com/oauth/userinfo";

/// `http://127.0.0.1:<port>/callback`.
pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

/// The client ID to use, if any: a non-empty `DDMM_NEXUS_CLIENT_ID` in the
/// environment (for testing), else the compiled-in one.
pub fn client_id() -> Option<String> {
    client_id_from(std::env::var(CLIENT_ID_ENV).ok().as_deref(), BUILD_CLIENT_ID)
}

fn client_id_from(runtime: Option<&str>, build: &str) -> Option<String> {
    runtime
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .or_else(|| Some(build.trim()).filter(|id| !id.is_empty()))
        .map(str::to_string)
}

/// Where the OAuth requests go. Production is [`Endpoints::nexus`]; tests
/// point it at a local mock.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub revoke: String,
    pub userinfo: String,
    /// Production: HTTPS only, and only to `users.nexusmods.com`.
    strict: bool,
}

impl Endpoints {
    pub fn nexus() -> Self {
        Self {
            authorize: AUTHORIZE_URL.into(),
            token: TOKEN_URL.into(),
            revoke: REVOKE_URL.into(),
            userinfo: USERINFO_URL.into(),
            strict: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn mock(base: &str) -> Self {
        Self {
            authorize: format!("{base}/oauth/authorize"),
            token: format!("{base}/oauth/token"),
            revoke: format!("{base}/oauth/revoke"),
            userinfo: format!("{base}/oauth/userinfo"),
            strict: false,
        }
    }

    /// Whether a token may be sent to `url`.
    fn allows(&self, url: &str) -> bool {
        if !self.strict {
            return true;
        }
        reqwest::Url::parse(url)
            .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some(USERS_HOST) && u.port().is_none())
    }
}

// ---------------------------------------------------------------- PKCE

/// A PKCE pair (RFC 7636): a random verifier and its S256 challenge.
pub struct Pkce {
    verifier: Secret,
    pub challenge: String,
}

impl Pkce {
    /// 48 random bytes, base64url without padding: a 64-character verifier
    /// drawn only from the unreserved characters RFC 7636 allows.
    pub fn generate() -> anyhow::Result<Self> {
        let mut bytes = [0u8; 48];
        SysRng.try_fill_bytes(&mut bytes)?;
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        Ok(Self { challenge: s256_challenge(&verifier), verifier: Secret::new(verifier) })
    }

    pub fn verifier(&self) -> &str {
        self.verifier.expose()
    }
}

/// `BASE64URL(SHA256(ASCII(verifier)))`, no padding.
pub fn s256_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A random, unguessable `state` value (256 bits).
pub fn random_state() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    SysRng.try_fill_bytes(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Constant-time comparison, so `state` can't be guessed byte by byte.
fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn authorize_url(endpoints: &Endpoints, client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(&endpoints.authorize)?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge);
    Ok(url.into())
}

// ---------------------------------------------------------------- errors

/// Why sign-in didn't complete. Every message is our own wording (plus, at
/// most, an OAuth error *code* from Nexus) -- never a token, code or
/// verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    NoClientId,
    AlreadyRunning,
    PortBusy(u16),
    Listener(String),
    Browser(String),
    TimedOut,
    Cancelled,
    StateMismatch,
    MissingCode,
    /// The user clicked "Deny" / Nexus said `access_denied`.
    Denied,
    /// Nexus redirected back with another OAuth error code.
    Provider(String),
    /// The token endpoint refused the code or answered unexpectedly.
    Exchange(String),
    Network(String),
    Storage(String),
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignInError::NoClientId => f.write_str("Signing in to Nexus Mods isn't available in this build yet. You can still enter a personal API key."),
            SignInError::AlreadyRunning => f.write_str("A Nexus Mods sign-in is already in progress."),
            SignInError::PortBusy(port) => write!(f, "Couldn't start sign-in: port {port} on this computer is already in use (or reserved). Close whatever is using port {port} and try again."),
            SignInError::Listener(e) => write!(f, "Couldn't start sign-in: {e}"),
            SignInError::Browser(e) => write!(f, "Couldn't open your browser for sign-in: {e}"),
            SignInError::TimedOut => f.write_str("Sign-in timed out after 5 minutes. Try again when you're ready."),
            SignInError::Cancelled => f.write_str("Sign-in was cancelled."),
            SignInError::StateMismatch => f.write_str("Sign-in was rejected because the reply didn't match this sign-in attempt. Try again."),
            SignInError::MissingCode => f.write_str("Nexus Mods didn't send back an authorization code. Try again."),
            SignInError::Denied => f.write_str("Sign-in was declined on Nexus Mods. Nothing was saved."),
            SignInError::Provider(code) => write!(f, "Nexus Mods reported a sign-in error ({code}). Try again."),
            SignInError::Exchange(e) => write!(f, "Nexus Mods didn't complete the sign-in: {e}"),
            SignInError::Network(e) => write!(f, "Couldn't reach Nexus Mods: {e}"),
            SignInError::Storage(e) => write!(f, "Signed in, but couldn't save the sign-in: {e}"),
        }
    }
}

impl std::error::Error for SignInError {}

// ---------------------------------------------------------------- callback

/// What a request to the loopback listener turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    /// Not our callback (e.g. `/favicon.ico`): answer 404 and keep waiting.
    NotCallback,
    Code(Secret),
    Failed(SignInError),
}

/// Parse the request target of `GET /callback?...` against the `state` we
/// sent. An `error` or a `state` mismatch never yields a code.
pub fn parse_callback(target: &str, expected_state: &str) -> Callback {
    let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::NotCallback;
    };
    if url.path() != CALLBACK_PATH {
        return Callback::NotCallback;
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if !state.as_deref().is_some_and(|s| same_secret(s, expected_state)) {
        return Callback::Failed(SignInError::StateMismatch);
    }
    if let Some(error) = error {
        return Callback::Failed(if error == "access_denied" {
            SignInError::Denied
        } else {
            // Only keep something that looks like an OAuth error code.
            let code: String = error.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(64).collect();
            SignInError::Provider(if code.is_empty() { "unknown_error".into() } else { code })
        });
    }
    match code {
        Some(c) if !c.is_empty() => Callback::Code(Secret::new(c)),
        _ => Callback::Failed(SignInError::MissingCode),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// The page the browser shows after the redirect.
pub fn callback_page(result: &Result<(), SignInError>) -> String {
    let (title, body) = match result {
        Ok(()) => (
            "Signed in to Nexus Mods",
            "You can close this tab and return to DDMM.".to_string(),
        ),
        Err(e) => ("Sign-in didn't complete", format!("{} You can close this tab and return to DDMM.", e)),
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title} - DDMM</title>\
<style>body{{background:#18181b;color:#e4e4e7;font-family:system-ui,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}}\
main{{text-align:center;max-width:32rem;padding:1rem}}h1{{font-size:1.4rem;color:{color}}}p{{color:#a1a1aa}}</style></head>\
<body><main><h1>{title}</h1><p>{body}</p><p>Democracy Defender Mod Manager</p></main></body></html>",
        title = html_escape(title),
        body = html_escape(&body),
        color = if result.is_ok() { "#4ade80" } else { "#f87171" },
    )
}

/// Read one HTTP request head (up to 16 KiB) and return its target, if it's
/// a `GET`.
async fn read_request_target(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let mut parts = head.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_string))?
}

async fn respond(sock: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

/// Bind the loopback listener: 127.0.0.1 only, never another port.
pub async fn bind_listener(port: u16) -> Result<TcpListener, SignInError> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await.map_err(|e| match e.kind() {
        // Windows reports a port inside a reserved (excluded) range as
        // "access denied" rather than "in use".
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => SignInError::PortBusy(port),
        _ => SignInError::Listener(e.to_string()),
    })
}

/// Serve the loopback listener until the callback arrives (or `cancel`
/// fires), answer it, and return the code. The listener is dropped -- and
/// the port closed -- when this returns.
pub async fn wait_for_callback(listener: TcpListener, expected_state: String, mut cancel: oneshot::Receiver<()>) -> Result<Secret, SignInError> {
    let (tx, mut rx) = mpsc::channel::<Result<Secret, SignInError>>(4);
    let mut tasks = tokio::task::JoinSet::new();
    let result = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((mut sock, peer)) = accepted else { continue };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let tx = tx.clone();
                let expected = expected_state.clone();
                // One task per connection, so an idle browser pre-connect
                // can't hold up the real request.
                tasks.spawn(async move {
                    let Some(target) = read_request_target(&mut sock).await else { return };
                    match parse_callback(&target, &expected) {
                        Callback::NotCallback => respond(&mut sock, "404 Not Found", "Not found").await,
                        Callback::Code(code) => {
                            respond(&mut sock, "200 OK", &callback_page(&Ok(()))).await;
                            let _ = tx.send(Ok(code)).await;
                        }
                        Callback::Failed(e) => {
                            respond(&mut sock, "400 Bad Request", &callback_page(&Err(e.clone()))).await;
                            let _ = tx.send(Err(e)).await;
                        }
                    }
                });
            }
            Some(outcome) = rx.recv() => break outcome,
            _ = &mut cancel => break Err(SignInError::Cancelled),
        }
    };
    drop(listener);
    tasks.abort_all();
    result
}

// ---------------------------------------------------------------- tokens

/// The stored sign-in. Serialized (as JSON) only into the OS keychain /
/// owner-only fallback file.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: Secret,
    #[serde(default)]
    pub refresh_token: Option<Secret>,
    /// Unix seconds.
    pub expires_at: i64,
    #[serde(default)]
    pub scope: String,
    /// Nexus Mods account name, for Settings.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub is_premium: bool,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at", &self.expires_at)
            .field("username", &self.username)
            .finish()
    }
}

impl Tokens {
    /// True when the access token expires within [`REFRESH_SKEW_SECS`].
    pub fn needs_refresh(&self, now: i64) -> bool {
        now + REFRESH_SKEW_SECS >= self.expires_at
    }

    pub fn is_expired(&self, now: i64) -> bool {
        now >= self.expires_at
    }

    fn secrets(&self) -> Vec<&str> {
        let mut v = vec![self.access_token.expose()];
        if let Some(r) = &self.refresh_token {
            v.push(r.expose());
        }
        v
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize, Default)]
struct OAuthErrorBody {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    membership_roles: Vec<String>,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The `exp` claim of a JWT access token, if it has one (read only as an
/// expiry hint, never trusted for anything else).
fn jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()?.get("exp")?.as_i64()
}

/// The `user.username` claim of a Nexus JWT access token, as a fallback
/// account name when userinfo is unavailable.
fn jwt_username(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("user")?.get("username")?.as_str().map(str::to_string)
}

fn http_client(endpoints: &Endpoints) -> Result<reqwest::Client, SignInError> {
    reqwest::Client::builder()
        .user_agent(crate::providers::user_agent())
        // Tokens must never follow a redirect anywhere.
        .redirect(reqwest::redirect::Policy::none())
        .https_only(endpoints.strict)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| SignInError::Network(e.to_string()))
}

/// `application/x-www-form-urlencoded` body (the OAuth token/revoke
/// endpoints take forms, never query strings, so nothing ends up in a URL).
fn form_body(req: reqwest::RequestBuilder, form: &[(&str, &str)]) -> reqwest::RequestBuilder {
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form).finish();
    req.header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(body)
}

fn app_headers(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    req.header("Application-Name", APP_NAME)
        .header("Application-Version", env!("CARGO_PKG_VERSION"))
        .header(reqwest::header::ACCEPT, "application/json")
}

/// Outcome of a token-endpoint call that failed.
#[derive(Debug, PartialEq, Eq)]
enum TokenFailure {
    /// 400/401/403: the code/refresh token isn't (or is no longer) valid.
    Rejected(String),
    Network(String),
}

async fn token_request(endpoints: &Endpoints, form: &[(&str, &str)], redact: &[&str]) -> Result<TokenResponse, TokenFailure> {
    if !endpoints.allows(&endpoints.token) {
        return Err(TokenFailure::Network("refusing to send credentials to an unexpected host".into()));
    }
    let http = http_client(endpoints).map_err(|e| TokenFailure::Network(e.to_string()))?;
    let response = form_body(app_headers(http.post(&endpoints.token)), form)
        .send()
        .await
        .map_err(|e| TokenFailure::Network(secrets::redact_all(&e.without_url().to_string(), redact)))?;
    let status = response.status();
    let body = crate::providers::read_capped(response)
        .await
        .map_err(|e| TokenFailure::Network(secrets::redact_all(&e.to_string(), redact)))?;
    if status.is_success() {
        return serde_json::from_str::<TokenResponse>(&body)
            .map_err(|_| TokenFailure::Network("unexpected response from the Nexus Mods token endpoint".into()));
    }
    // Only the OAuth error *code* is kept -- never the body, which could
    // echo what we sent.
    let code = serde_json::from_str::<OAuthErrorBody>(&body)
        .unwrap_or_default()
        .error
        .map(|e| e.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(64).collect::<String>())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
    match status.as_u16() {
        400 | 401 | 403 => Err(TokenFailure::Rejected(code)),
        _ => Err(TokenFailure::Network(code)),
    }
}

fn tokens_from(resp: TokenResponse, previous_refresh: Option<&Secret>, now: i64) -> Tokens {
    let expires_at = resp
        .expires_in
        .filter(|s| *s > 0)
        .map(|s| now + s)
        .or_else(|| jwt_exp(&resp.access_token))
        .unwrap_or(now + DEFAULT_LIFETIME_SECS);
    Tokens {
        username: jwt_username(&resp.access_token),
        access_token: Secret::new(resp.access_token),
        // A refresh response may omit the refresh token: keep the old one.
        refresh_token: resp.refresh_token.filter(|r| !r.is_empty()).map(Secret::new).or_else(|| previous_refresh.cloned()),
        expires_at,
        scope: resp.scope.unwrap_or_else(|| SCOPES.to_string()),
        is_premium: false,
    }
}

/// Fill in the account name (and premium flag) from the userinfo endpoint.
/// Best effort: a failure keeps the name from the token, if any.
async fn fetch_user_info(endpoints: &Endpoints, tokens: &mut Tokens) {
    if !endpoints.allows(&endpoints.userinfo) {
        return;
    }
    let Ok(http) = http_client(endpoints) else { return };
    let result = app_headers(http.get(&endpoints.userinfo))
        .bearer_auth(tokens.access_token.expose())
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => match crate::providers::read_capped(resp)
            .await
            .ok()
            .and_then(|body| serde_json::from_str::<UserInfo>(&body).ok())
        {
            Some(info) => {
                if let Some(name) = info.name.filter(|n| !n.trim().is_empty()) {
                    tokens.username = Some(name);
                }
                tokens.is_premium = info.membership_roles.iter().any(|r| r == "premium" || r == "lifetimepremium");
            }
            None => log::warn!("Nexus Mods userinfo answered with an unexpected format."),
        },
        Ok(resp) => log::warn!("Nexus Mods userinfo answered {}.", resp.status()),
        Err(e) => log::warn!("Couldn't reach the Nexus Mods userinfo endpoint: {}", secrets::redact_all(&e.without_url().to_string(), &tokens.secrets())),
    }
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(endpoints: &Endpoints, client_id: &str, code: &Secret, verifier: &str, redirect_uri: &str) -> Result<Tokens, SignInError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code.expose()),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ];
    let now = now_unix();
    let resp = token_request(endpoints, &form, &[code.expose(), verifier]).await.map_err(|f| match f {
        TokenFailure::Rejected(c) => SignInError::Exchange(format!("the sign-in was refused ({c})")),
        TokenFailure::Network(e) => SignInError::Network(e),
    })?;
    let mut tokens = tokens_from(resp, None, now);
    fetch_user_info(endpoints, &mut tokens).await;
    Ok(tokens)
}

#[derive(Debug, PartialEq, Eq)]
pub enum RefreshError {
    /// Nexus refused the refresh token: the user revoked DDMM's access (or
    /// it expired). The sign-in is gone.
    Revoked,
    /// Couldn't refresh right now (network, server error); try later.
    Temporary(String),
}

/// Get a new access token with the refresh token.
pub async fn refresh(endpoints: &Endpoints, client_id: &str, tokens: &Tokens) -> Result<Tokens, RefreshError> {
    let Some(refresh_token) = tokens.refresh_token.as_ref() else {
        return Err(RefreshError::Revoked);
    };
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token.expose()),
    ];
    let now = now_unix();
    let resp = token_request(endpoints, &form, &tokens.secrets()).await.map_err(|f| match f {
        TokenFailure::Rejected(_) => RefreshError::Revoked,
        TokenFailure::Network(e) => RefreshError::Temporary(e),
    })?;
    let mut new = tokens_from(resp, Some(refresh_token), now);
    // Keep what we know about the account.
    if new.username.is_none() {
        new.username = tokens.username.clone();
    }
    new.is_premium = tokens.is_premium;
    Ok(new)
}

/// Ask Nexus to revoke the sign-in (best effort; RFC 7009 `revoke`
/// endpoint from the discovery document).
pub async fn revoke(endpoints: &Endpoints, client_id: &str, tokens: &Tokens) {
    if !endpoints.allows(&endpoints.revoke) {
        return;
    }
    let Ok(http) = http_client(endpoints) else { return };
    let (token, hint) = match &tokens.refresh_token {
        Some(r) => (r.expose(), "refresh_token"),
        None => (tokens.access_token.expose(), "access_token"),
    };
    let form = [("token", token), ("token_type_hint", hint), ("client_id", client_id)];
    match form_body(app_headers(http.post(&endpoints.revoke)), &form).send().await {
        Ok(r) if r.status().is_success() => log::info!("Nexus Mods sign-in revoked."),
        Ok(r) => log::warn!("Nexus Mods didn't confirm revoking the sign-in ({}); it was still removed from this computer.", r.status()),
        Err(e) => log::warn!(
            "Couldn't reach Nexus Mods to revoke the sign-in ({}); it was still removed from this computer.",
            secrets::redact_all(&e.without_url().to_string(), &tokens.secrets())
        ),
    }
}

// ---------------------------------------------------------------- storage

pub async fn load_tokens(base_path: &Path) -> Option<(Tokens, KeyStorage)> {
    let (raw, storage) = secrets::load_secret(base_path, OAUTH_SLOT).await?;
    match serde_json::from_str::<Tokens>(&raw) {
        Ok(t) if !t.access_token.is_empty() => Some((t, storage)),
        _ => {
            log::warn!("The stored Nexus Mods sign-in couldn't be read; ignoring it.");
            None
        }
    }
}

pub async fn store_tokens(base_path: &Path, tokens: &Tokens) -> anyhow::Result<KeyStorage> {
    let json = serde_json::to_string(tokens)?;
    secrets::store_secret(base_path, OAUTH_SLOT, &json)
        .await
        .map_err(|e| anyhow::anyhow!(secrets::redact_all(&e.to_string(), &tokens.secrets())))
}

/// Forget the sign-in on this computer (no network).
pub async fn remove_tokens(base_path: &Path) {
    secrets::remove_secret(base_path, OAUTH_SLOT).await;
    // Cached Nexus decisions belong to whatever account made them.
    let _ = tokio::fs::remove_file(base_path.join("update-cache.json")).await;
}

// ---------------------------------------------------------------- sign-in

/// Everything one sign-in attempt needs.
pub struct SignInConfig {
    pub endpoints: Endpoints,
    pub client_id: String,
    pub port: u16,
    pub timeout: Duration,
}

impl SignInConfig {
    pub fn production(client_id: String) -> Self {
        Self { endpoints: Endpoints::nexus(), client_id, port: REDIRECT_PORT, timeout: SIGN_IN_TIMEOUT }
    }
}

/// Run the whole browser sign-in; `open_browser` is handed the authorize
/// URL (it contains no secret -- only the PKCE *challenge* and `state`).
pub async fn sign_in<F>(cfg: &SignInConfig, open_browser: F, cancel: oneshot::Receiver<()>) -> Result<Tokens, SignInError>
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let pkce = Pkce::generate().map_err(|e| SignInError::Listener(e.to_string()))?;
    let state = random_state().map_err(|e| SignInError::Listener(e.to_string()))?;
    let listener = bind_listener(cfg.port).await?;
    let port = listener.local_addr().map_err(|e| SignInError::Listener(e.to_string()))?.port();
    let redirect = redirect_uri(port);
    let url = authorize_url(&cfg.endpoints, &cfg.client_id, &redirect, &state, &pkce.challenge)
        .map_err(|e| SignInError::Listener(e.to_string()))?;

    log::info!("Nexus Mods sign-in: waiting for the browser on 127.0.0.1:{port}.");
    open_browser(&url).map_err(SignInError::Browser)?;

    let code = match tokio::time::timeout(cfg.timeout, wait_for_callback(listener, state, cancel)).await {
        Ok(result) => result?,
        Err(_) => return Err(SignInError::TimedOut),
    };
    log::info!("Nexus Mods sign-in: authorization received; exchanging it for tokens.");
    exchange_code(&cfg.endpoints, &cfg.client_id, &code, pkce.verifier(), &redirect).await
}

// ---------------------------------------------------------------- auth provider

fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Which credential a Nexus request should use, before any refresh.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice<T, K> {
    OAuth(T),
    ApiKey(K),
    None,
}

/// Precedence: signed in > personal API key > nothing.
pub fn choose<T, K>(signed_in: Option<T>, key: Option<K>) -> Choice<T, K> {
    match (signed_in, key) {
        (Some(t), _) => Choice::OAuth(t),
        (None, Some(k)) => Choice::ApiKey(k),
        (None, None) => Choice::None,
    }
}

/// The result of [`resolve_auth`].
#[derive(Debug)]
pub enum ResolvedAuth {
    Auth(NexusAuth),
    /// Neither signed in nor a key: Nexus is "optional: sign in or add a
    /// key to check".
    NoCredentials,
    /// The sign-in was revoked or expired and has been removed; the message
    /// tells the user.
    SignedOut(String),
    /// Couldn't get a usable token right now (e.g. offline during refresh).
    Unavailable(String),
}

pub const SIGNED_OUT_MESSAGE: &str = "Your Nexus Mods sign-in is no longer valid (it may have been revoked), so DDMM signed you out. Sign in again in Settings to check Nexus mods.";

/// The single auth provider for every Nexus API request: returns the
/// credential to use, refreshing the access token first if it's about to
/// expire. Only call it for user-initiated actions (it may contact
/// `users.nexusmods.com` to refresh).
pub async fn resolve_auth(base_path: &Path) -> ResolvedAuth {
    resolve_auth_with(base_path, &Endpoints::nexus(), client_id(), now_unix()).await
}

pub(crate) async fn resolve_auth_with(base_path: &Path, endpoints: &Endpoints, client_id: Option<String>, now: i64) -> ResolvedAuth {
    let _guard = refresh_lock().lock().await;
    let signed_in = load_tokens(base_path).await.map(|(t, _)| t);
    let key = if signed_in.is_none() { secrets::load(base_path).await.map(|(k, _)| k) } else { None };
    let tokens = match choose(signed_in, key) {
        Choice::OAuth(t) => t,
        Choice::ApiKey(k) => return ResolvedAuth::Auth(NexusAuth::ApiKey(k)),
        Choice::None => return ResolvedAuth::NoCredentials,
    };
    if !tokens.needs_refresh(now) {
        return ResolvedAuth::Auth(NexusAuth::OAuth(tokens.access_token));
    }
    let Some(client_id) = client_id else {
        // Can't refresh without a client ID; use the token while it lasts.
        if !tokens.is_expired(now) {
            return ResolvedAuth::Auth(NexusAuth::OAuth(tokens.access_token));
        }
        log::warn!("The Nexus Mods sign-in expired and this build can't refresh it; signing out.");
        remove_tokens(base_path).await;
        return ResolvedAuth::SignedOut(SIGNED_OUT_MESSAGE.into());
    };
    log::info!("Refreshing the Nexus Mods sign-in (access token expires soon).");
    match refresh(endpoints, &client_id, &tokens).await {
        Ok(new) => {
            if let Err(e) = store_tokens(base_path, &new).await {
                log::warn!("Couldn't save the refreshed Nexus Mods sign-in: {e}");
            }
            ResolvedAuth::Auth(NexusAuth::OAuth(new.access_token))
        }
        Err(RefreshError::Revoked) => {
            log::warn!("Nexus Mods refused to refresh the sign-in (revoked or expired); signing out.");
            remove_tokens(base_path).await;
            ResolvedAuth::SignedOut(SIGNED_OUT_MESSAGE.into())
        }
        Err(RefreshError::Temporary(e)) => {
            log::warn!("Couldn't refresh the Nexus Mods sign-in right now: {e}");
            if !tokens.is_expired(now) {
                ResolvedAuth::Auth(NexusAuth::OAuth(tokens.access_token))
            } else {
                ResolvedAuth::Unavailable(format!("Couldn't renew your Nexus Mods sign-in right now ({e}). Try again later."))
            }
        }
    }
}

/// Called when the Nexus API rejects a signed-in request (401/403): the
/// sign-in was revoked. Remove it and return the message for the user.
pub async fn handle_rejected_sign_in(base_path: &Path) -> String {
    log::warn!("Nexus Mods rejected the signed-in request (revoked?); signing out.");
    remove_tokens(base_path).await;
    SIGNED_OUT_MESSAGE.to_string()
}

/// Whether any Nexus credential is stored (local only, no network).
pub async fn has_credentials(base_path: &Path) -> bool {
    load_tokens(base_path).await.is_some() || secrets::load(base_path).await.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 7636 Appendix B.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn s256_matches_the_rfc_7636_test_vector() {
        assert_eq!(s256_challenge(RFC_VERIFIER), RFC_CHALLENGE);
    }

    #[test]
    fn pkce_verifier_has_the_right_length_and_charset() {
        let a = Pkce::generate().unwrap();
        let b = Pkce::generate().unwrap();
        for p in [&a, &b] {
            let v = p.verifier();
            assert!((43..=128).contains(&v.len()), "{}", v.len());
            assert_eq!(v.len(), 64);
            assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)), "{v}");
            assert_eq!(p.challenge, s256_challenge(v));
            assert_eq!(p.challenge.len(), 43);
            assert!(!p.challenge.contains('='));
        }
        assert_ne!(a.verifier(), b.verifier());
    }

    #[test]
    fn state_is_random_and_compared_exactly() {
        let a = random_state().unwrap();
        assert_eq!(a.len(), 43);
        assert_ne!(a, random_state().unwrap());
        assert!(same_secret(&a, &a.clone()));
        assert!(!same_secret(&a, &a[..42]));
        assert!(!same_secret(&a, &format!("{}x", &a[..42])));
        assert!(!same_secret(&a, ""));
    }

    #[test]
    fn redirect_uri_is_the_registered_one() {
        assert_eq!(redirect_uri(REDIRECT_PORT), "http://127.0.0.1:28647/callback");
        assert_ne!(REDIRECT_PORT, 28635);
    }

    #[test]
    fn client_id_prefers_a_non_empty_runtime_value() {
        assert_eq!(client_id_from(None, ""), None);
        assert_eq!(client_id_from(Some(""), ""), None);
        assert_eq!(client_id_from(Some("  "), "built"), Some("built".into()));
        assert_eq!(client_id_from(None, "built"), Some("built".into()));
        assert_eq!(client_id_from(Some("env-id"), "built"), Some("env-id".into()));
    }

    #[test]
    fn authorize_url_has_every_parameter() {
        let url = authorize_url(&Endpoints::nexus(), "ddmm-test", &redirect_uri(REDIRECT_PORT), "st4te", RFC_CHALLENGE).unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some(USERS_HOST));
        assert_eq!(parsed.path(), "/oauth/authorize");
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "ddmm-test");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["scope"], "openid public");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:28647/callback");
        assert_eq!(q["state"], "st4te");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], RFC_CHALLENGE);
        assert!(!q.contains_key("code_verifier"));
    }

    #[test]
    fn callback_success() {
        match parse_callback("/callback?code=abc123&state=S1", "S1") {
            Callback::Code(c) => assert_eq!(c.expose(), "abc123"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn callback_errors() {
        assert_eq!(parse_callback("/callback?error=access_denied&state=S1", "S1"), Callback::Failed(SignInError::Denied));
        assert_eq!(
            parse_callback("/callback?error=server_error&error_description=%3Cscript%3E&state=S1", "S1"),
            Callback::Failed(SignInError::Provider("server_error".into()))
        );
        assert_eq!(
            parse_callback("/callback?error=%3Cb%3Ex&state=S1", "S1"),
            Callback::Failed(SignInError::Provider("bx".into()))
        );
    }

    #[test]
    fn callback_wrong_or_missing_state() {
        assert_eq!(parse_callback("/callback?code=abc&state=WRONG", "S1"), Callback::Failed(SignInError::StateMismatch));
        assert_eq!(parse_callback("/callback?code=abc", "S1"), Callback::Failed(SignInError::StateMismatch));
        // An error with the wrong state is still a state mismatch (never
        // trust anything from a reply that isn't ours).
        assert_eq!(parse_callback("/callback?error=access_denied&state=X", "S1"), Callback::Failed(SignInError::StateMismatch));
    }

    #[test]
    fn callback_missing_code() {
        assert_eq!(parse_callback("/callback?state=S1", "S1"), Callback::Failed(SignInError::MissingCode));
        assert_eq!(parse_callback("/callback?code=&state=S1", "S1"), Callback::Failed(SignInError::MissingCode));
    }

    #[test]
    fn other_paths_are_not_the_callback() {
        assert_eq!(parse_callback("/favicon.ico", "S1"), Callback::NotCallback);
        assert_eq!(parse_callback("/callbackx?code=a&state=S1", "S1"), Callback::NotCallback);
        assert_eq!(parse_callback("/", "S1"), Callback::NotCallback);
    }

    #[test]
    fn callback_page_escapes_and_says_close_the_tab() {
        let ok = callback_page(&Ok(()));
        assert!(ok.contains("You can close this tab and return to DDMM."));
        let err = callback_page(&Err(SignInError::Provider("<x>".into())));
        assert!(!err.contains("<x>"));
    }

    fn tokens(expires_at: i64) -> Tokens {
        Tokens {
            access_token: Secret::new("ACCESS-SECRET"),
            refresh_token: Some(Secret::new("REFRESH-SECRET")),
            expires_at,
            scope: SCOPES.into(),
            username: Some("Diver".into()),
            is_premium: false,
        }
    }

    #[test]
    fn refresh_timing() {
        let now = 1_000_000;
        assert!(!tokens(now + 3600).needs_refresh(now));
        assert!(!tokens(now + REFRESH_SKEW_SECS + 1).needs_refresh(now));
        assert!(tokens(now + REFRESH_SKEW_SECS).needs_refresh(now), "refresh 5 minutes early");
        assert!(tokens(now + 10).needs_refresh(now));
        assert!(!tokens(now + 10).is_expired(now));
        assert!(tokens(now - 1).needs_refresh(now));
        assert!(tokens(now).is_expired(now));
    }

    #[test]
    fn expiry_comes_from_expires_in_then_the_jwt() {
        let now = 1_000;
        let resp = |exp_in, token: &str| TokenResponse { access_token: token.into(), refresh_token: None, expires_in: exp_in, scope: None };
        assert_eq!(tokens_from(resp(Some(21600), "opaque"), None, now).expires_at, now + 21600);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":5000,"user":{"username":"JwtDiver"}}"#);
        let jwt = format!("h.{payload}.s");
        let t = tokens_from(resp(None, &jwt), Some(&Secret::new("old-refresh")), now);
        assert_eq!(t.expires_at, 5000);
        assert_eq!(t.username.as_deref(), Some("JwtDiver"));
        assert_eq!(t.refresh_token.unwrap().expose(), "old-refresh", "kept when the response omits it");
        assert_eq!(tokens_from(resp(None, "opaque"), None, now).expires_at, now + DEFAULT_LIFETIME_SECS);
    }

    #[test]
    fn precedence_signed_in_then_key_then_nothing() {
        assert_eq!(choose(Some("tok"), Some("key")), Choice::OAuth("tok"));
        assert_eq!(choose(Some("tok"), None::<&str>), Choice::OAuth("tok"));
        assert_eq!(choose(None::<&str>, Some("key")), Choice::ApiKey("key"));
        assert_eq!(choose(None::<&str>, None::<&str>), Choice::None);
    }

    #[test]
    fn tokens_never_show_up_in_debug_or_errors() {
        let t = tokens(0);
        let printed = format!("{t:?}");
        assert!(!printed.contains("ACCESS-SECRET") && !printed.contains("REFRESH-SECRET"), "{printed}");
        let msg = secrets::redact_all("bad ACCESS-SECRET and REFRESH-SECRET", &t.secrets());
        assert_eq!(msg, "bad [redacted] and [redacted]");
        for e in [
            SignInError::Exchange("the sign-in was refused (invalid_grant)".into()),
            SignInError::PortBusy(REDIRECT_PORT),
            SignInError::TimedOut,
        ] {
            assert!(!e.to_string().contains("SECRET"));
        }
        assert!(SignInError::PortBusy(REDIRECT_PORT).to_string().contains("Close whatever is using port 28647"));
    }

    #[test]
    fn production_endpoints_only_allow_users_nexusmods_over_https() {
        let e = Endpoints::nexus();
        for url in [&e.authorize, &e.token, &e.revoke, &e.userinfo] {
            assert!(e.allows(url), "{url}");
        }
        assert!(!e.allows("http://users.nexusmods.com/oauth/token"));
        assert!(!e.allows("https://evil.example/oauth/token"));
        assert!(!e.allows("https://users.nexusmods.com.evil.example/oauth/token"));
        assert!(!e.allows("https://users.nexusmods.com:8443/oauth/token"));
    }

    #[tokio::test]
    async fn busy_port_is_reported_clearly_and_not_worked_around() {
        let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        match bind_listener(port).await {
            Err(SignInError::PortBusy(p)) => assert_eq!(p, port),
            Err(other) => panic!("{other:?}"),
            Ok(l) => panic!("bound a busy port: {:?}", l.local_addr()),
        }
    }

    #[tokio::test]
    async fn listener_times_out_and_closes() {
        let listener = bind_listener(0).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_tx, rx) = oneshot::channel();
        let r = tokio::time::timeout(Duration::from_millis(200), wait_for_callback(listener, "S".into(), rx)).await;
        assert!(r.is_err(), "no callback, so it must time out");
        // The future (and with it the listener) is gone: the port is closed.
        assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    }

    #[tokio::test]
    async fn listener_can_be_cancelled() {
        let listener = bind_listener(0).await.unwrap();
        let (tx, rx) = oneshot::channel();
        tx.send(()).unwrap();
        assert_eq!(wait_for_callback(listener, "S".into(), rx).await.unwrap_err(), SignInError::Cancelled);
    }

    #[tokio::test]
    async fn listener_ignores_other_paths_then_takes_the_callback() {
        let listener = bind_listener(0).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_tx, rx) = oneshot::channel();
        let wait = tokio::spawn(wait_for_callback(listener, "S1".into(), rx));
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let fav = http.get(format!("http://127.0.0.1:{port}/favicon.ico")).send().await.unwrap();
        assert_eq!(fav.status(), 404);
        let page = http.get(format!("http://127.0.0.1:{port}/callback?code=C0DE&state=S1")).send().await.unwrap();
        assert_eq!(page.status(), 200);
        assert!(page.text().await.unwrap().contains("You can close this tab and return to DDMM."));
        assert_eq!(wait.await.unwrap().unwrap().expose(), "C0DE");
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err(), "listener closed right after");
    }

    #[tokio::test]
    async fn listener_rejects_a_wrong_state() {
        let listener = bind_listener(0).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_tx, rx) = oneshot::channel();
        let wait = tokio::spawn(wait_for_callback(listener, "S1".into(), rx));
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let page = http.get(format!("http://127.0.0.1:{port}/callback?code=C0DE&state=EVIL")).send().await.unwrap();
        assert_eq!(page.status(), 400);
        assert_eq!(wait.await.unwrap().unwrap_err(), SignInError::StateMismatch);
    }
}

/// End-to-end against a local mock of the Nexus OAuth server and API:
/// authorize -> redirect to the loopback listener -> token exchange ->
/// userinfo -> refresh -> an API call carrying the Bearer header.
#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use crate::providers::nexus::NexusClient;

    #[derive(Default)]
    struct MockState {
        /// (path, headers, body) of every request.
        requests: Vec<(String, HashMap<String, String>, String)>,
        codes: HashMap<String, (String, String)>, // code -> (challenge, redirect)
        issued: u32,
        refuse_refresh: bool,
        expires_in: i64,
    }

    fn form(body: &str) -> HashMap<String, String> {
        url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
    }

    fn reply(status: &str, headers: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    /// A tiny mock of users.nexusmods.com + api.nexusmods.com.
    async fn mock_nexus(state: Arc<Mutex<MockState>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let state = state.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        if let Some(i) = text.find("\r\n\r\n") {
                            let head = text[..i].to_string();
                            let len = head
                                .lines()
                                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                                .unwrap_or(0);
                            if buf.len() >= i + 4 + len {
                                break (head, text[i + 4..i + 4 + len].to_string());
                            }
                        }
                    };
                    let mut lines = head.lines();
                    let target = lines.next().unwrap().split_whitespace().nth(1).unwrap().to_string();
                    let headers: HashMap<String, String> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let path = target.split('?').next().unwrap().to_string();
                    let resp = {
                        let mut st = state.lock().unwrap();
                        st.requests.push((path.clone(), headers.clone(), body.clone()));
                        match path.as_str() {
                            "/oauth/authorize" => {
                                let q: HashMap<String, String> = reqwest::Url::parse(&format!("http://x{target}"))
                                    .unwrap()
                                    .query_pairs()
                                    .into_owned()
                                    .collect();
                                assert_eq!(q["client_id"], "fake-client-id");
                                assert_eq!(q["code_challenge_method"], "S256");
                                assert_eq!(q["scope"], "openid public");
                                let code = format!("CODE-{}", st.codes.len() + 1);
                                st.codes.insert(code.clone(), (q["code_challenge"].clone(), q["redirect_uri"].clone()));
                                let mut loc = reqwest::Url::parse(&q["redirect_uri"]).unwrap();
                                loc.query_pairs_mut().append_pair("code", &code).append_pair("state", &q["state"]);
                                reply("302 Found", &format!("Location: {loc}\r\n"), "")
                            }
                            "/oauth/token" => {
                                let f = form(&body);
                                assert_eq!(f["client_id"], "fake-client-id");
                                let ok = match f["grant_type"].as_str() {
                                    "authorization_code" => {
                                        let (challenge, redirect) = st.codes.remove(&f["code"]).expect("unknown or reused code");
                                        assert_eq!(s256_challenge(&f["code_verifier"]), challenge, "PKCE verifier mismatch");
                                        assert_eq!(f["redirect_uri"], redirect);
                                        true
                                    }
                                    "refresh_token" => !st.refuse_refresh && f["refresh_token"].starts_with("REFRESH-"),
                                    other => panic!("grant {other}"),
                                };
                                if ok {
                                    st.issued += 1;
                                    let n = st.issued;
                                    let exp = st.expires_in;
                                    reply(
                                        "200 OK",
                                        "Content-Type: application/json\r\n",
                                        &format!(r#"{{"access_token":"ACCESS-{n}","refresh_token":"REFRESH-{n}","expires_in":{exp},"token_type":"Bearer","scope":"openid public"}}"#),
                                    )
                                } else {
                                    reply("400 Bad Request", "Content-Type: application/json\r\n", r#"{"error":"invalid_grant","error_description":"echo REFRESH-1"}"#)
                                }
                            }
                            "/oauth/userinfo" => {
                                assert!(headers["authorization"].starts_with("Bearer ACCESS-"));
                                reply("200 OK", "Content-Type: application/json\r\n", r#"{"sub":"4242","name":"MockDiver","membership_roles":["member","premium"]}"#)
                            }
                            "/oauth/revoke" => reply("200 OK", "", ""),
                            "/v1/users/validate.json" => {
                                if headers.get("authorization").is_some_and(|a| a.starts_with("Bearer ACCESS-")) {
                                    reply("200 OK", "Content-Type: application/json\r\n", r#"{"user_id":4242,"name":"MockDiver","is_premium":true}"#)
                                } else {
                                    reply("401 Unauthorized", "Content-Type: application/json\r\n", r#"{"message":"no"}"#)
                                }
                            }
                            _ => reply("404 Not Found", "", ""),
                        }
                    };
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        base
    }

    /// Stands in for the system browser: follows the authorize redirect to
    /// the loopback listener, like a real browser would.
    fn fake_browser(url: &str) -> Result<(), String> {
        let url = url.to_string();
        tokio::spawn(async move {
            let http = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::limited(3)).build().unwrap();
            let page = http.get(url).send().await.unwrap();
            assert_eq!(page.status(), 200);
            assert!(page.text().await.unwrap().contains("You can close this tab"));
        });
        Ok(())
    }

    #[tokio::test]
    async fn full_sign_in_refresh_and_bearer_api_call_against_a_mock_server() {
        // The runtime override, the way a tester would supply a client ID.
        std::env::set_var(CLIENT_ID_ENV, "fake-client-id");
        let client_id = client_id().expect("env override");
        assert_eq!(client_id, "fake-client-id");

        let state = Arc::new(Mutex::new(MockState { expires_in: 21600, ..Default::default() }));
        let base = mock_nexus(state.clone()).await;
        let cfg = SignInConfig { endpoints: Endpoints::mock(&base), client_id: client_id.clone(), port: 0, timeout: Duration::from_secs(20) };

        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let tokens = sign_in(&cfg, fake_browser, cancel_rx).await.expect("sign-in");
        assert_eq!(tokens.access_token.expose(), "ACCESS-1");
        assert_eq!(tokens.refresh_token.as_ref().unwrap().expose(), "REFRESH-1");
        assert_eq!(tokens.username.as_deref(), Some("MockDiver"));
        assert!(tokens.is_premium);
        assert!(tokens.expires_at > now_unix() + 21000);

        // Stored (fallback file in a temp dir when no keychain is around).
        let dir = tempfile::tempdir().unwrap();
        store_tokens(dir.path(), &tokens).await.unwrap();
        assert_eq!(load_tokens(dir.path()).await.unwrap().0, tokens);

        // Fresh token: used as-is (no refresh request).
        let before = state.lock().unwrap().requests.len();
        let auth = match resolve_auth_with(dir.path(), &cfg.endpoints, Some(client_id.clone()), now_unix()).await {
            ResolvedAuth::Auth(a) => a,
            other => panic!("{other:?}"),
        };
        assert_eq!(state.lock().unwrap().requests.len(), before);
        assert_eq!(auth, NexusAuth::OAuth(Secret::new("ACCESS-1")));

        // Close to expiry: refreshed first, and the new tokens are stored.
        let later = tokens.expires_at - 60;
        let auth = match resolve_auth_with(dir.path(), &cfg.endpoints, Some(client_id.clone()), later).await {
            ResolvedAuth::Auth(a) => a,
            other => panic!("{other:?}"),
        };
        assert_eq!(auth, NexusAuth::OAuth(Secret::new("ACCESS-2")));
        assert_eq!(load_tokens(dir.path()).await.unwrap().0.refresh_token.unwrap().expose(), "REFRESH-2");

        // The API call carries the Bearer token + app headers, not apikey.
        let mut api = NexusClient::with_base(auth, &format!("{base}/v1/")).unwrap();
        assert_eq!(api.validate().await.unwrap().name, "MockDiver");
        {
            let st = state.lock().unwrap();
            let (_, headers, _) = st.requests.iter().rev().find(|(p, _, _)| p == "/v1/users/validate.json").unwrap();
            assert_eq!(headers["authorization"], "Bearer ACCESS-2");
            assert!(!headers.contains_key("apikey"));
            assert_eq!(headers["application-name"], APP_NAME);
            assert_eq!(headers["application-version"], env!("CARGO_PKG_VERSION"));
            // The token endpoint got a form post with the verifier, never
            // the verifier in a URL.
            assert!(st.requests.iter().filter(|(p, _, _)| p == "/oauth/authorize").count() == 1);
        }

        // Revoked: refresh refused -> signed out cleanly, with a message.
        state.lock().unwrap().refuse_refresh = true;
        let expired = load_tokens(dir.path()).await.unwrap().0.expires_at + 10;
        match resolve_auth_with(dir.path(), &cfg.endpoints, Some(client_id.clone()), expired).await {
            ResolvedAuth::SignedOut(msg) => {
                assert!(msg.contains("signed you out"));
                assert!(!msg.contains("REFRESH-"));
            }
            other => panic!("{other:?}"),
        }
        assert!(load_tokens(dir.path()).await.is_none(), "tokens deleted locally");
        // With a personal key also stored, it takes over after sign-out.
        secrets::store(dir.path(), &secrets::NexusApiKey::parse("PERSONAL-KEY").unwrap()).await.unwrap();
        assert!(matches!(
            resolve_auth_with(dir.path(), &cfg.endpoints, Some(client_id.clone()), expired).await,
            ResolvedAuth::Auth(NexusAuth::ApiKey(_))
        ));
        secrets::remove(dir.path()).await;
        assert!(matches!(
            resolve_auth_with(dir.path(), &cfg.endpoints, Some(client_id), expired).await,
            ResolvedAuth::NoCredentials
        ));
    }

    #[tokio::test]
    async fn a_refused_refresh_means_revoked() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let base = mock_nexus(state).await;
        let t = Tokens {
            access_token: Secret::new("ACCESS-x"),
            refresh_token: Some(Secret::new("NOT-A-REFRESH")),
            expires_at: 0,
            scope: String::new(),
            username: None,
            is_premium: false,
        };
        let err = refresh(&Endpoints::mock(&base), "fake-client-id", &t).await.unwrap_err();
        assert_eq!(err, RefreshError::Revoked);
    }
}
