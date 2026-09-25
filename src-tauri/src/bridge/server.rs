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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

use crate::{
    commands::{profiles::do_load_profiles, settings::do_load_settings},
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
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let mut handshake_line = String::new();
    if reader.read_line(&mut handshake_line).await? == 0 {
        return Ok(());
    }
    let handshake: serde_json::Value = serde_json::from_str(handshake_line.trim()).unwrap_or_default();
    let supplied = handshake.get("hello").and_then(|v| v.as_str()).unwrap_or("");
    if !tokens_match(&expected_token, supplied) {
        let _ = write_half_write(&mut write_half, "{\"ok\":false}").await;
        return Ok(());
    }
    write_half_write(&mut write_half, "{\"ok\":true}").await?;

    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let reply = dispatch(&app, trimmed).await;
        write_half_write(&mut write_half, &reply).await?;
    }

    Ok(())
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

    match kind.as_str() {
        "hello" => handle_hello(app, id).await,
        "install" => handle_install_queued(app, raw, id).await,
        "query" => handle_query(app, raw, id).await,
        "status" => handle_status(app, id).await,
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

async fn handle_status(app: &AppHandle, id: String) -> String {
    let state = app.state::<AppState>();
    let settings = do_load_settings(&state.base_path).await.ok();
    let game_found = match &settings {
        Some(s) => s.validate().await.is_ok(),
        None => false,
    };
    let mod_count = state.mods.lock().await.as_ref().map(Vec::len).unwrap_or(0);
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
    let mods_guard = state.mods.lock().await;
    let mods = mods_guard.as_deref().unwrap_or(&[]);

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
            let update_available = match (&installed_version, &req.page_version) {
                (Some(installed), Some(latest)) if !latest.is_empty() => Some(installed != latest),
                _ => None,
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

    let depth = state.bridge_queue_depth.fetch_add(1, Ordering::SeqCst) + 1;
    if depth > MAX_QUEUE_DEPTH {
        state.bridge_queue_depth.fetch_sub(1, Ordering::SeqCst);
        return ErrorReply::new(id, ErrorCode::Busy, "too many installs queued, try again shortly").to_line();
    }

    let _permit = state.bridge_install_lock.lock().await;
    let result = handle_install(app, raw, id.clone()).await;
    state.bridge_queue_depth.fetch_sub(1, Ordering::SeqCst);
    result
}

async fn handle_install(app: &AppHandle, raw: serde_json::Value, id: String) -> String {
    let req: InstallRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => return ErrorReply::new(id, ErrorCode::BadRequest, e.to_string()).to_line(),
    };

    let site = req
        .page_url
        .as_deref()
        .and_then(registrable_domain_of_url)
        .or_else(|| req.download_url.as_deref().and_then(registrable_domain_of_url));

    if let Some(site) = &site {
        if !is_site_allowed(app, site).await {
            match request_consent(app, &id, site).await {
                ConsentDecision::Deny => {
                    return ErrorReply::new(id, ErrorCode::Declined, "You chose not to install this mod.").to_line();
                }
                ConsentDecision::AlwaysAllow => {
                    allow_site(app, site).await;
                }
                ConsentDecision::JustOnce => {}
            }
        }
    }

    let state = app.state::<AppState>();
    let file = std::path::PathBuf::from(&req.file);

    let outcome = {
        let mut mods_guard = state.mods.lock().await;
        let Some(mods) = mods_guard.as_mut() else {
            return ErrorReply::new(id, ErrorCode::Internal, "mods not read").to_line();
        };
        install::install_file(
            &state,
            mods,
            &file,
            req.page_url.as_deref(),
            req.download_url.as_deref(),
            req.page_version.as_deref(),
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

    let completion = request_install_completion(app, &id, &outcome, &after_install).await;

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
            source: outcome.r#mod.sources.first().map(|s| InstalledModSource {
                provider: s.provider.clone(),
                id: None,
            }),
            version: outcome.r#mod.sources.first().and_then(|s| s.version.clone()),
        },
        updated: outcome.updated,
        added_to_profile: completion.added_to_profile,
        deployed: completion.deployed,
        warnings,
    };
    serde_json::to_string(&reply).unwrap()
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
    if let Ok(mut settings) = do_load_settings(&state.base_path).await {
        settings.allow_bridge_site(site.to_string());
        if let Ok(data) = serde_json::to_vec_pretty(&settings) {
            let _ = tokio::fs::write(state.base_path.join("settings.json"), data).await;
        }
    }
}

/// Ask the frontend to show the per-site consent prompt and bring the
/// window to the front, then wait for the user's answer. Times out to
/// `Deny` -- a closed/ignored prompt must never fall through to installing.
async fn request_consent(app: &AppHandle, request_id: &str, site: &str) -> ConsentDecision {
    let (tx, rx) = oneshot::channel();
    {
        let state = app.state::<AppState>();
        state.bridge_pending.lock().await.consent.insert(request_id.to_string(), tx);
    }

    focus_main_window(app);

    let _ = app.emit(
        "bridge://consent-request",
        serde_json::json!({ "requestId": request_id, "site": site }),
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
    request_id: &str,
    outcome: &install::InstallOutcome,
    after_install: &str,
) -> InstallCompletion {
    let (tx, rx) = oneshot::channel();
    {
        let state = app.state::<AppState>();
        state
            .bridge_pending
            .lock()
            .await
            .install_completion
            .insert(request_id.to_string(), tx);
    }

    let payload = serde_json::json!({
        "requestId": request_id,
        "mod": {
            "guid": outcome.r#mod.guid().to_string(),
            "name": outcome.r#mod.name(),
        },
        "afterInstall": after_install,
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

fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
