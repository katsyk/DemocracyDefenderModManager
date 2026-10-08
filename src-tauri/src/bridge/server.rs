//! The in-app half of the bridge: a loopback TCP listener the native
//! messaging host relays browser-extension requests through. See
//! `docs/development/bridge-protocol.md`.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::atomic::Ordering,
    time::Duration,
};

use tauri::{AppHandle, Emitter, Manager};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

use crate::{
    commands::{mods::ensure_mods_loaded, profiles::do_load_profiles, settings::do_load_settings},
    AppState,
};

use super::{
    allowlist::{is_allowed_origin, registrable_domain_of_url},
    install,
    protocol::*,
    state::{generate_token, remove_bridge_file, tokens_match, write_bridge_file, BridgeInfo},
    ConsentDecision, InstallCompletion,
};

const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);
const INSTALL_COMPLETION_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an install waits for the Mods page to be ready to handle the
/// consent prompt / afterInstall step -- mostly a cold start, where the
/// install arrives while the webview is still loading.
const FRONTEND_READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Wait (up to `FRONTEND_READY_TIMEOUT`) until the frontend can handle
/// bridge events. Emitting before that loses the event: nothing is
/// listening yet, so a consent prompt would never appear (and time out as
/// Deny) and the afterInstall step would silently not happen.
async fn wait_for_frontend(app: &AppHandle) -> bool {
    let rx = app.state::<AppState>().bridge_frontend_ready.subscribe();
    wait_until_ready(rx, FRONTEND_READY_TIMEOUT).await
}

/// `true` as soon as `rx` holds `true` (immediately if it already does),
/// `false` if that doesn't happen within `timeout`.
async fn wait_until_ready(mut rx: tokio::sync::watch::Receiver<bool>, timeout: Duration) -> bool {
    let ready = tokio::time::timeout(timeout, async { rx.wait_for(|ready| *ready).await.is_ok() }).await;
    matches!(ready, Ok(true))
}

/// Start the bridge: bind a loopback listener on a random port, write
/// `bridge.json`, and start accepting connections in the background.
/// Returns once the listener is up and the file is written.
pub async fn start(app: AppHandle) -> anyhow::Result<()> {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
    let port = listener.local_addr()?.port();
    let token = generate_token()?;

    let base_path = app.state::<AppState>().base_path.clone();
    write_bridge_file(&base_path, &BridgeInfo::new(port, token.clone())).await?;
    log::info!("Bridge listening on 127.0.0.1:{port}");

    tokio::spawn(accept_loop(app, listener, token));

    Ok(())
}

/// Remove `bridge.json` on clean shutdown, per the security checklist.
pub async fn shutdown(app: &AppHandle) {
    let base_path = app.state::<AppState>().base_path.clone();
    remove_bridge_file(&base_path).await;
}

async fn accept_loop(app: AppHandle, listener: TcpListener, token: String) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let app = app.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(app, stream, token).await {
                        log::debug!("Bridge connection ended: {e}");
                    }
                });
            }
            Err(e) => {
                log::error!("Bridge listener accept failed, stopping: {e}");
                break;
            }
        }
    }
}

async fn handle_connection(app: AppHandle, stream: TcpStream, expected_token: String) -> anyhow::Result<()> {
    serve_connection(stream, expected_token, move |line| {
        let app = app.clone();
        async move { dispatch(&app, &line).await }
    })
    .await
}

/// How many requests from one connection may be in progress at once;
/// beyond that a request is answered `BUSY` at once. (The extension never
/// gets near it: it sends a handful of requests at a time.)
const MAX_IN_FLIGHT_PER_CONNECTION: usize = 32;

/// The unauthenticated handshake (`{"hello":"<64 hex chars>"}`) must fit in
/// this many bytes and arrive within [`HANDSHAKE_TIMEOUT`].
const MAX_HANDSHAKE_BYTES: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// One authenticated connection from the host relay: the token handshake,
/// then full duplex. Every request is handled on its own task, and its
/// reply is written as soon as it's ready, echoing the request's `id` (the
/// host routes replies by `id`). So a `hello`/`query`/`status` is answered
/// right away even while an install waits minutes for the user's consent;
/// installs themselves still run one at a time (`handle_install_queued`).
async fn serve_connection<H, F>(stream: TcpStream, expected_token: String, handle: H) -> anyhow::Result<()>
where
    H: Fn(String) -> F,
    F: std::future::Future<Output = String> + Send + 'static,
{
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Nothing is trusted before the token: the handshake line is small and
    // must come promptly, or the connection is dropped.
    let handshake_line =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, read_bounded_line(&mut reader, MAX_HANDSHAKE_BYTES)).await {
            Err(_) | Ok(Ok(None)) => return Ok(()),
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(Some(BoundedLine::Line(line)))) => line,
            Ok(Ok(Some(BoundedLine::TooLong))) => String::new(),
        };
    let handshake: serde_json::Value = serde_json::from_str(handshake_line.trim()).unwrap_or_default();
    let supplied = handshake.get("hello").and_then(|v| v.as_str()).unwrap_or("");
    if !tokens_match(&expected_token, supplied) {
        let _ = write_half_write(&mut write_half, "{\"ok\":false}").await;
        return Ok(());
    }
    write_half_write(&mut write_half, "{\"ok\":true}").await?;

    // Replies are written by one task, in whatever order they're ready.
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(reply) = reply_rx.recv().await {
            if write_half_write(&mut write_half, &reply).await.is_err() {
                break;
            }
        }
    });

    let in_flight = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT_PER_CONNECTION));
    let result = loop {
        let line = match read_bounded_line(&mut reader, MAX_LINE_BYTES).await {
            Ok(None) => break Ok(()),
            Err(e) => break Err(e.into()),
            Ok(Some(BoundedLine::TooLong)) => {
                let _ = reply_tx.send(ErrorReply::new("", ErrorCode::BadRequest, "message too large").to_line());
                continue;
            }
            Ok(Some(BoundedLine::Line(line))) => line,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(permit) = in_flight.clone().try_acquire_owned() else {
            let id = request_id(&line);
            let _ = reply_tx.send(ErrorReply::new(id, ErrorCode::Busy, "too many requests at once, try again shortly").to_line());
            continue;
        };
        let reply = handle(line.trim().to_string());
        let reply_tx = reply_tx.clone();
        tokio::spawn(async move {
            let _ = reply_tx.send(reply.await);
            drop(permit);
        });
    };

    // Requests already started still finish (an install isn't abandoned
    // halfway because the browser went away); the writer ends once they
    // have all replied.
    drop(reply_tx);
    let _ = writer.await;
    result
}

/// The `id` of a request line, or `""` when it has none.
fn request_id(line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .unwrap_or_default()
}

async fn write_half_write(w: &mut (impl tokio::io::AsyncWrite + Unpin), line: &str) -> anyhow::Result<()> {
    w.write_all(line.as_bytes()).await?;
    w.write_all(b"\n").await?;
    Ok(())
}

async fn dispatch(app: &AppHandle, line: &str) -> String {
    let raw: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return ErrorReply::new("", ErrorCode::BadRequest, format!("malformed JSON: {e}")).to_line(),
    };

    let id = raw.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let kind = raw.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // The host relay sets `origin`; the browser itself can't (see
    // host::relay). Absence or an unrecognized value is always rejected.
    let origin = raw.get("origin").and_then(|v| v.as_str()).unwrap_or("");
    if !is_allowed_origin(origin) {
        return ErrorReply::new(id, ErrorCode::ForbiddenOrigin, "caller is not an allowed extension").to_line();
    }

    // Any authenticated request from the extension means it's installed and
    // connected -- a browser-based mod update can then finish with its
    // "Update with DDMM" button (see commands::updates).
    *app.state::<AppState>().bridge_last_seen.lock().await = Some(tokio::time::Instant::now());

    match kind.as_str() {
        "hello" => handle_hello(app, id).await,
        "install" => handle_install_queued(app, raw, id).await,
        "query" => handle_query(app, raw, id).await,
        "status" => handle_status(app, id).await,
        "open" => handle_open(app, id),
        other => ErrorReply::new(id, ErrorCode::Unsupported, format!("unknown message type \"{other}\"")).to_line(),
    }
}

async fn handle_hello(app: &AppHandle, id: String) -> String {
    let state = app.state::<AppState>();
    let settings = do_load_settings(&state.base_path).await.ok();
    let game_found = match &settings {
        Some(s) => s.validate().await.is_ok(),
        None => false,
    };
    let after_install = settings
        .as_ref()
        .map(|s| s.after_browser_install().to_string())
        .unwrap_or_else(|| "deploy".to_string());

    let reply = HelloReply {
        id,
        ok: true,
        kind: "hello",
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: PROTOCOL_VERSION,
        after_install,
        game_found,
    };
    serde_json::to_string(&reply).unwrap()
}

/// `open`: bring DDMM's window to the front (the extension popup's "Start
/// DDMM" button). Installs nothing, so no consent prompt; the origin
/// allowlist above still applies.
fn handle_open(app: &AppHandle, id: String) -> String {
    focus_main_window(app);
    serde_json::to_string(&OpenedReply { id, ok: true, kind: "opened" }).unwrap()
}

async fn handle_status(app: &AppHandle, id: String) -> String {
    let state = app.state::<AppState>();
    let settings = do_load_settings(&state.base_path).await.ok();
    let game_found = match &settings {
        Some(s) => s.validate().await.is_ok(),
        None => false,
    };
    let mod_count = {
        let mut mods_guard = state.mods.lock().await;
        ensure_mods_loaded(&mut mods_guard, &state.base_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    };
    let active_profile = do_load_profiles(&state.base_path)
        .await
        .ok()
        .and_then(|p| p.profiles.get(p.active as usize).map(|pr| pr.name().to_string()));
    let busy = state.bridge_queue_depth.load(Ordering::SeqCst) > 0;

    let reply = StatusReply {
        id,
        ok: true,
        kind: "status",
        game_found,
        active_profile,
        mod_count,
        busy,
    };
    serde_json::to_string(&reply).unwrap()
}

async fn handle_query(app: &AppHandle, raw: serde_json::Value, id: String) -> String {
    let req: QueryRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => return ErrorReply::new(id, ErrorCode::BadRequest, e.to_string()).to_line(),
    };

    let Some(source) = install::resolve_source(Some(&req.page_url), None, req.page_version.as_deref()) else {
        return ErrorReply::new(id, ErrorCode::BadRequest, "pageUrl could not be resolved").to_line();
    };

    let state = app.state::<AppState>();
    let mut mods_guard = state.mods.lock().await;
    let mods: &[crate::models::Mod] = match ensure_mods_loaded(&mut mods_guard, &state.base_path).await {
        Ok(m) => m,
        Err(e) => return ErrorReply::new(id, ErrorCode::Internal, format!("couldn't read installed mods: {e}")).to_line(),
    };

    let found = source.id.as_ref().and_then(|source_id| {
        mods.iter().find(|m| {
            m.sources.iter().any(|s| {
                s.provider.eq_ignore_ascii_case(&source.provider)
                    && s.page_url
                        .as_deref()
                        .and_then(crate::sources::source_from_page_url)
                        .and_then(|r| r.id)
                        .as_deref()
                        == Some(source_id.as_str())
            })
        })
    });

    let (installed, r#mod, update_available) = match found {
        Some(m) => {
            let installed_version = m
                .sources
                .iter()
                .find(|s| s.provider.eq_ignore_ascii_case(&source.provider))
                .and_then(|s| s.version.clone());
            let by_version = match (&installed_version, &req.page_version) {
                (Some(installed), Some(latest)) if !latest.is_empty() => Some(
                    crate::providers::compare_versions(installed, latest) == crate::providers::VersionRelation::Update,
                ),
                _ => None,
            };
            // DDMM's own update check found one for this mod: not a guess.
            // Covers pages that don't expose a version (Nexus) and pages
            // whose version didn't change for a same-day update (AyakaMods
            // versions are often just the date).
            let update_available = if by_version != Some(true)
                && crate::commands::updates::known_update_available(&state, m.guid(), &source.provider).await
            {
                Some(true)
            } else {
                by_version
            };
            (
                true,
                Some(QueryModSummary {
                    guid: m.guid().to_string(),
                    name: m.name().to_string(),
                    installed_version,
                }),
                update_available,
            )
        }
        None => (false, None, None),
    };

    let reply = QueryReply {
        id,
        ok: true,
        kind: "queryResult",
        installed,
        r#mod,
        update_available,
    };
    serde_json::to_string(&reply).unwrap()
}

async fn handle_install_queued(app: &AppHandle, raw: serde_json::Value, id: String) -> String {
    let state = app.state::<AppState>();

    let Some(_queued) = QueueSlot::take(&state.bridge_queue_depth, MAX_QUEUE_DEPTH) else {
        return ErrorReply::new(id, ErrorCode::Busy, "too many installs queued, try again shortly").to_line();
    };
    let _recent = RecentFileGuard::new(&state, &raw);

    let _permit = state.bridge_install_lock.lock().await;
    let Ok(_data_op) = state.data_op() else {
        return ErrorReply::new(
            id,
            ErrorCode::Busy,
            "DDMM is moving its data folder; try again once it has restarted",
        )
        .to_line();
    };
    handle_install(app, raw, id.clone()).await
}

/// One place in the install queue (`bridge_queue_depth`), given back when
/// dropped -- however the install ends, a panic included, so a failed
/// install can never leave the queue looking permanently fuller.
struct QueueSlot<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> QueueSlot<'a> {
    /// A place in the queue, or `None` when `max` are already taken.
    fn take(depth: &'a std::sync::atomic::AtomicUsize, max: usize) -> Option<Self> {
        let slot = QueueSlot(depth);
        (depth.fetch_add(1, Ordering::SeqCst) < max).then_some(slot)
    }
}

impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Keeps auto-import from offering `file` while the extension's install of
/// it runs, and for a while after (see `RecentBridgeFiles`).
struct RecentFileGuard<'a> {
    state: &'a AppState,
    file: Option<std::path::PathBuf>,
}

impl<'a> RecentFileGuard<'a> {
    fn new(state: &'a AppState, raw: &serde_json::Value) -> Self {
        let file = raw.get("file").and_then(|f| f.as_str()).map(std::path::PathBuf::from);
        if let Some(file) = &file {
            state.bridge_recent_files.begin(file);
        }
        Self { state, file }
    }
}

impl Drop for RecentFileGuard<'_> {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            self.state.bridge_recent_files.finish(file);
        }
    }
}

async fn handle_install(app: &AppHandle, raw: serde_json::Value, id: String) -> String {
    let req: InstallRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => return ErrorReply::new(id, ErrorCode::BadRequest, e.to_string()).to_line(),
    };

    let site = consent_site(req.page_url.as_deref(), req.download_url.as_deref());
    let site_allowed = match &site {
        Some(site) => is_site_allowed(app, site).await,
        None => false,
    };
    if needs_consent(site.as_deref(), site_allowed) {
        if !wait_for_frontend(app).await {
            return ErrorReply::new(
                id,
                ErrorCode::Internal,
                "DDMM couldn't show the permission prompt. Open DDMM's Mods page and try again.",
            )
            .to_line();
        }
        let file_name = std::path::Path::new(&req.file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match request_consent(app, site.as_deref(), &file_name).await {
            ConsentDecision::Deny => {
                return ErrorReply::new(id, ErrorCode::Declined, "You chose not to install this mod.").to_line();
            }
            ConsentDecision::AlwaysAllow => {
                // With no site there's nothing to remember: this once only.
                if let Some(site) = &site {
                    allow_site(app, site).await;
                }
            }
            ConsentDecision::JustOnce => {}
        }
    }

    let state = app.state::<AppState>();
    let file = std::path::PathBuf::from(&req.file);

    // Record what's being installed (version, which file) so update checks
    // work for it: from the page, or -- before taking the mods lock, since
    // it may ask the site -- from the archive name / the site itself.
    let mut page_version = req.page_version.clone();
    let mut installed_files = Vec::new();
    if let Some(source) = install::resolve_source(req.page_url.as_deref(), req.download_url.as_deref(), page_version.as_deref())
        .filter(|s| s.id.is_some())
    {
        let (enriched, files) =
            crate::commands::updates::enrich_install_source(&source, Some(&file), req.download_url.as_deref()).await;
        // Starts from the page's version; enrichment only fills it in, or
        // replaces it with the tag a GitHub release-asset link names.
        page_version = enriched.version;
        installed_files = files;
    }

    let outcome = {
        let mut mods_guard = state.mods.lock().await;
        let mods = match ensure_mods_loaded(&mut mods_guard, &state.base_path).await {
            Ok(m) => m,
            Err(e) => {
                return ErrorReply::new(id, ErrorCode::Internal, format!("couldn't read installed mods: {e}")).to_line()
            }
        };
        install::install_file_with(
            &state,
            mods,
            &file,
            req.page_url.as_deref(),
            req.download_url.as_deref(),
            page_version.as_deref(),
            installed_files,
        )
        .await
    };

    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return ErrorReply::new(id, e.code, e.message).to_line(),
    };

    let settings = do_load_settings(&state.base_path).await.ok();
    let after_install = req
        .after_install
        .clone()
        .or_else(|| settings.as_ref().map(|s| s.after_browser_install().to_string()))
        .unwrap_or_else(|| "deploy".to_string());

    let completion = if wait_for_frontend(app).await {
        request_install_completion(app, &outcome, &after_install).await
    } else {
        log::warn!("Frontend never became ready; skipping the afterInstall step for a bridge install");
        InstallCompletion {
            warnings: vec!["Installed to the library only: DDMM's Mods page wasn't open to add it to a profile.".to_string()],
            ..InstallCompletion::default()
        }
    };

    let mut warnings = completion.warnings.clone();
    if let Some(w) = &outcome.warning {
        warnings.push(w.clone());
    }

    if let Some((code, message)) = completion.soft_error {
        // The mod is still installed; the spec calls for a success-shaped
        // error here so the extension can tell "installed but ..." apart
        // from an outright failure.
        return ErrorReply::new(id, code, message).to_line();
    }

    let reply = InstalledReply {
        id,
        ok: true,
        kind: "installed",
        installed_mod: InstalledModSummary {
            guid: outcome.r#mod.guid().to_string(),
            name: outcome.r#mod.name().to_string(),
            source: outcome.source.as_ref().map(|s| InstalledModSource {
                provider: s.provider.clone(),
                id: s.id.clone(),
            }),
            version: outcome.source.as_ref().and_then(|s| s.version.clone()),
        },
        updated: outcome.updated,
        added_to_profile: completion.added_to_profile,
        deployed: completion.deployed,
        warnings,
    };
    serde_json::to_string(&reply).unwrap()
}

/// The site (registrable domain) an extension install is asked about and
/// can be "Always allow"ed for: from `pageUrl`, else `downloadUrl`, and only
/// from an `http(s)` URL.
fn consent_site(page_url: Option<&str>, download_url: Option<&str>) -> Option<String> {
    page_url
        .and_then(registrable_domain_of_url)
        .or_else(|| download_url.and_then(registrable_domain_of_url))
}

/// Every extension install needs the user's consent: "Always allow" for its
/// site, or an answer to the prompt now. An install with no web URL at all
/// has no site that could have been allowed, so it is always asked about
/// (and the prompt offers no "Always allow" for it).
fn needs_consent(site: Option<&str>, site_allowed: bool) -> bool {
    site.is_none() || !site_allowed
}

async fn is_site_allowed(app: &AppHandle, site: &str) -> bool {
    let state = app.state::<AppState>();
    do_load_settings(&state.base_path)
        .await
        .map(|s| s.is_bridge_site_allowed(site))
        .unwrap_or(false)
}

async fn allow_site(app: &AppHandle, site: &str) {
    let state = app.state::<AppState>();
    let _data_op = match state.data_op() {
        Ok(op) => op,
        Err(e) => {
            log::warn!("Couldn't save \"Always allow\" for {site}: {e:#}");
            return;
        }
    };
    let saved = crate::commands::settings::update_settings(&state.base_path, |settings| {
        settings.allow_bridge_site(site.to_string());
        Ok(())
    })
    .await;
    if let Err(e) = saved {
        log::error!("Couldn't save \"Always allow\" for {site}: {e:#}");
    }
}

/// Ask the frontend to show the per-site consent prompt and bring the
/// window to the front, then wait for the user's answer. Times out to
/// `Deny` -- a closed/ignored prompt must never fall through to installing.
///
/// The round trip is keyed by a fresh server-generated key, not by the
/// extension's request `id` (which restarts at "1" whenever the browser
/// restarts the extension), so a late answer to an old prompt can never
/// resolve a newer install's.
async fn request_consent(app: &AppHandle, site: Option<&str>, file_name: &str) -> ConsentDecision {
    let (request_id, rx) = {
        let state = app.state::<AppState>();
        let mut pending = state.bridge_pending.lock().await;
        pending.wait_for_consent()
    };
    let request_id = request_id.as_str();

    focus_main_window(app);

    let _ = app.emit(
        "bridge://consent-request",
        serde_json::json!({ "requestId": request_id, "site": site, "fileName": file_name }),
    );

    match tokio::time::timeout(CONSENT_TIMEOUT, rx).await {
        Ok(Ok(decision)) => decision,
        _ => {
            let state = app.state::<AppState>();
            state.bridge_pending.lock().await.consent.remove(request_id);
            ConsentDecision::Deny
        }
    }
}

/// Ask the frontend to add the freshly-installed mod to the active profile
/// and/or deploy it, per `afterInstall`, then wait for it to report back
/// what actually happened.
async fn request_install_completion(
    app: &AppHandle,
    outcome: &install::InstallOutcome,
    after_install: &str,
) -> InstallCompletion {
    // Keyed by a fresh key, for the same reason as `request_consent`.
    let (request_id, rx) = {
        let state = app.state::<AppState>();
        let mut pending = state.bridge_pending.lock().await;
        pending.wait_for_completion()
    };
    let request_id = request_id.as_str();

    let payload = serde_json::json!({
        "requestId": request_id,
        "mod": {
            "guid": outcome.r#mod.guid().to_string(),
            "name": outcome.r#mod.name(),
        },
        "afterInstall": after_install,
        "updated": outcome.updated,
    });
    let _ = app.emit("bridge://mod-installed", payload);

    match tokio::time::timeout(INSTALL_COMPLETION_TIMEOUT, rx).await {
        Ok(Ok(completion)) => completion,
        _ => {
            let state = app.state::<AppState>();
            state.bridge_pending.lock().await.install_completion.remove(request_id);
            log::warn!("Timed out waiting for the frontend to finish a bridge install's afterInstall step");
            InstallCompletion::default()
        }
    }
}

/// Bring the main window to front -- reused by `deep_link` and by the
/// frontend-facing `commands::bridge::focus_main_window` command.
pub fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connect to a `serve_connection` running `handle`, past the handshake.
    async fn serve_for_test<H, F>(handle: H) -> (BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf)
    where
        H: Fn(String) -> F + Send + 'static,
        F: std::future::Future<Output = String> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = serve_connection(stream, "secret".into(), handle).await;
        });
        let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write.write_all(b"{\"hello\":\"secret\"}\n").await.unwrap();
        assert_eq!(read_line(&mut read).await, r#"{"ok":true}"#);
        (read, write)
    }

    async fn read_line(r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> String {
        match tokio::time::timeout(Duration::from_secs(5), read_bounded_line(r, MAX_LINE_BYTES)).await {
            Ok(Ok(Some(BoundedLine::Line(l)))) => l,
            other => panic!("expected a line, got {other:?}"),
        }
    }

    /// While an install waits (for the consent prompt, the frontend, the
    /// install queue), `hello`/`query`/`status` on the same connection are
    /// answered at once, and the install's reply still arrives later.
    #[tokio::test]
    async fn requests_are_answered_while_an_install_waits() {
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let (mut read, mut write) = serve_for_test(move |line| {
            let gate = gate.clone();
            async move {
                let id = request_id(&line);
                if line.contains("\"install\"") {
                    gate.notified().await;
                }
                format!(r#"{{"id":"{id}","ok":true}}"#)
            }
        })
        .await;

        write.write_all(b"{\"id\":\"1\",\"type\":\"install\"}\n").await.unwrap();
        write.write_all(b"{\"id\":\"2\",\"type\":\"hello\"}\n").await.unwrap();
        write.write_all(b"{\"id\":\"3\",\"type\":\"status\"}\n").await.unwrap();
        let mut early = vec![read_line(&mut read).await, read_line(&mut read).await];
        early.sort();
        assert_eq!(early, vec![r#"{"id":"2","ok":true}"#, r#"{"id":"3","ok":true}"#]);

        release.notify_one();
        assert_eq!(read_line(&mut read).await, r#"{"id":"1","ok":true}"#);
    }

    #[tokio::test]
    async fn a_wrong_token_is_refused_and_overlong_lines_are_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = serve_connection(stream, "secret".into(), |_| async { String::from("{}") }).await;
        });
        let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write.write_all(b"{\"hello\":\"wrong\"}\n").await.unwrap();
        assert_eq!(read_line(&mut read).await, r#"{"ok":false}"#);

        let (mut read, mut write) = serve_for_test(|line| async move { format!(r#"{{"id":"{}"}}"#, request_id(&line)) }).await;
        let mut big = vec![b'x'; MAX_LINE_BYTES + 10];
        big.push(b'\n');
        write.write_all(&big).await.unwrap();
        write.write_all(b"{\"id\":\"after\"}\n").await.unwrap();
        assert!(read_line(&mut read).await.contains("message too large"));
        assert_eq!(read_line(&mut read).await, r#"{"id":"after"}"#);
    }

    #[test]
    fn a_queue_slot_is_given_back_even_when_the_install_panics() {
        let depth = std::sync::atomic::AtomicUsize::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = QueueSlot::take(&depth, 2).unwrap();
            assert_eq!(depth.load(Ordering::SeqCst), 1);
            panic!("install blew up");
        }));
        assert!(result.is_err());
        assert_eq!(depth.load(Ordering::SeqCst), 0);

        let a = QueueSlot::take(&depth, 2).unwrap();
        let _b = QueueSlot::take(&depth, 2).unwrap();
        assert!(QueueSlot::take(&depth, 2).is_none(), "full");
        assert_eq!(depth.load(Ordering::SeqCst), 2, "a refused slot isn't counted");
        drop(a);
        assert!(QueueSlot::take(&depth, 2).is_some());
    }

    #[tokio::test]
    async fn an_oversized_or_silent_handshake_is_dropped() {
        for mode in ["oversized", "silent"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                serve_connection(stream, "secret".into(), |_| async { String::from("{}") }).await
            });
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            if mode == "oversized" {
                let mut line = format!("{{\"hello\":\"secret\",\"pad\":\"{}\"}}", "x".repeat(MAX_HANDSHAKE_BYTES));
                line.push('\n');
                stream.write_all(line.as_bytes()).await.unwrap();
            }
            // Refused (or timed out) without ever authenticating.
            let started = tokio::time::Instant::now();
            tokio::time::timeout(HANDSHAKE_TIMEOUT + Duration::from_secs(2), server).await.unwrap().unwrap().unwrap();
            if mode == "silent" {
                assert!(started.elapsed() >= HANDSHAKE_TIMEOUT - Duration::from_millis(100));
            }
            let mut buf = String::new();
            let mut reader = BufReader::new(stream);
            let _ = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut buf).await;
            assert!(!buf.contains("\"ok\":true"), "{mode}: {buf}");
        }
    }

    #[test]
    fn installs_without_a_web_url_always_need_consent() {
        for (page, download) in [
            (None, None),
            (Some("file:///home/me/Downloads/mod.zip"), None),
            (None, Some("ftp://files.example.com/mod.zip")),
            (Some("not a url"), Some("blob:https://example.com/1234")),
        ] {
            let site = consent_site(page, download);
            assert_eq!(site, None, "{page:?} {download:?}");
            // Even if something claimed to allow it, there is nothing to
            // have allowed.
            assert!(needs_consent(site.as_deref(), true));
        }
        let site = consent_site(Some("https://www.ayakamods.com/mods/x.4084/"), None);
        assert_eq!(site.as_deref(), Some("ayakamods.com"));
        assert!(needs_consent(site.as_deref(), false));
        assert!(!needs_consent(site.as_deref(), true));
    }

    #[tokio::test]
    async fn wait_until_ready_returns_at_once_when_already_ready() {
        let tx = tokio::sync::watch::Sender::new(true);
        assert!(wait_until_ready(tx.subscribe(), Duration::from_millis(10)).await);
    }

    #[tokio::test]
    async fn wait_until_ready_waits_for_the_frontend() {
        let tx = tokio::sync::watch::Sender::new(false);
        let rx = tx.subscribe();
        let waiter = tokio::spawn(wait_until_ready(rx, Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "must not proceed before the frontend is ready");
        tx.send_replace(true);
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn wait_until_ready_times_out() {
        let tx = tokio::sync::watch::Sender::new(false);
        assert!(!wait_until_ready(tx.subscribe(), Duration::from_millis(30)).await);
    }
}
