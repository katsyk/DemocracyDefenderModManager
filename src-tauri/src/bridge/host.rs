//! Host mode: `ddmm` started as the native-messaging host by a browser,
//! instead of as the normal desktop app. Detected from argv before Tauri
//! (or any window) is ever touched -- see `lib.rs::run`. This module never
//! opens a GUI; it only relays.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::FutureExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
};

use super::{
    allowlist::FIREFOX_EXTENSION_ID,
    protocol::{ErrorCode, ErrorReply, MAX_MESSAGE_BYTES},
    state::{read_bridge_file, tokens_match},
};

/// Which browser shape argv matched, carrying the exact `origin` string the
/// app expects on every forwarded request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOrigin(pub String);

/// Chromium browsers launch the host with `chrome-extension://<id>/` as the
/// first argument (Windows may append `--parent-window=<n>`). Firefox
/// launches it with `<path to host manifest> <extension id>`. Neither
/// shape depends on argv[0] (the exe path itself), so callers should pass
/// `std::env::args().collect::<Vec<_>>()` including it.
pub fn detect_host_mode(args: &[String]) -> Option<HostOrigin> {
    if let Some(a) = args.get(1) {
        if let Some(id) = a.strip_prefix("chrome-extension://") {
            let id = id.trim_end_matches('/');
            if !id.is_empty() {
                return Some(HostOrigin(format!("chrome-extension://{id}/")));
            }
        }
    }
    if args.get(2).map(String::as_str) == Some(FIREFOX_EXTENSION_ID) {
        return Some(HostOrigin(FIREFOX_EXTENSION_ID.to_string()));
    }
    None
}

fn spawn_detached(exe: &Path) -> io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    let mut child = cmd.spawn()?;
    // Reap it once it exits. The browser can keep this host alive for a
    // long time, and an un-waited child the user later closes would
    // otherwise linger as a zombie until then.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Read `bridge.json`, connect, and run the `{"hello": token}` handshake.
/// `None` for anything short of a fully successful handshake (missing
/// file, connection refused, wrong token, ...) -- every failure mode here
/// is treated identically: "not reachable right now".
async fn try_connect(base_path: &Path) -> Option<TcpStream> {
    let info = read_bridge_file(base_path).await.ok()?;
    let mut stream = TcpStream::connect(("127.0.0.1", info.port)).await.ok()?;

    stream
        .write_all(format!("{{\"hello\":\"{}\"}}\n", info.token).as_bytes())
        .await
        .ok()?;

    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.ok()?;
    let ack: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let ok = ack.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);

    // Belt and braces: the app already closes the connection on a bad
    // token, but double check the ack is unambiguous rather than assuming.
    if ok && tokens_match(&info.token, &info.token) {
        Some(stream)
    } else {
        None
    }
}

/// How long to wait for a freshly launched DDMM to write `bridge.json`.
/// Generous on purpose: a cold start on a busy machine (first launch after
/// boot, antivirus scanning the exe, WebView2 warming up) can take tens of
/// seconds, and giving up early turns the user's one click into two.
pub const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(45);

/// Only these request types may start DDMM when it isn't running: both are
/// explicit user actions (an install the user clicked for, or "Start DDMM"
/// in the extension popup). `hello`/`query`/`status` are sent just by
/// browsing a mod page, and browsing must never pop DDMM open.
pub fn may_launch_app(request_type: &str) -> bool {
    matches!(request_type, "install" | "open")
}

/// Don't launch DDMM again within this long of the last launch: a slow
/// start (it can take longer than the poll timeout on a busy machine) must
/// not turn every retry into yet another launch.
const RELAUNCH_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Launcher {
    last_launch: Option<tokio::time::Instant>,
}

impl Launcher {
    /// Reach the running app, starting it (detached, no window is opened by
    /// *this* process either way -- the launched instance runs normally) and
    /// polling for up to `poll_timeout` if it isn't already up.
    async fn ensure_app_running_and_connect(&mut self, resolve_base: fn() -> PathBuf, poll_timeout: Duration) -> Option<TcpStream> {
        if let Some(stream) = try_connect(&resolve_base()).await {
            return Some(stream);
        }

        let recently_launched = self.last_launch.is_some_and(|t| t.elapsed() < RELAUNCH_COOLDOWN);
        if !recently_launched {
            let exe = super::exe_path();
            if let Err(e) = spawn_detached(&exe) {
                log::error!("Failed to launch DDMM ({exe:?}): {e}");
                return None;
            }
            self.last_launch = Some(tokio::time::Instant::now());
        }

        let deadline = tokio::time::Instant::now() + poll_timeout;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if let Some(stream) = try_connect(&resolve_base()).await {
                return Some(stream);
            }
        }
        None
    }
}

enum Frame {
    Data(Vec<u8>),
    TooLarge,
    Eof,
}

/// Native messaging framing: a 4-byte **native-endian** (little-endian on
/// every platform this app targets) length, then that many bytes of UTF-8
/// JSON. A frame over `max` is drained (not just refused) so the stream
/// stays in sync for whatever the caller sends next.
async fn read_frame(reader: &mut (impl AsyncRead + Unpin), max: u32) -> io::Result<Frame> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(Frame::Eof),
        Err(e) => return Err(e),
    }

    let len = u32::from_le_bytes(len_buf);
    if len > max {
        let mut remaining = len as u64;
        let mut buf = [0u8; 8192];
        while remaining > 0 {
            let chunk = remaining.min(buf.len() as u64) as usize;
            reader.read_exact(&mut buf[..chunk]).await?;
            remaining -= chunk as u64;
        }
        return Ok(Frame::TooLarge);
    }

    let mut data = vec![0u8; len as usize];
    reader.read_exact(&mut data).await?;
    Ok(Frame::Data(data))
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> io::Result<()> {
    writer.write_all(&(data.len() as u32).to_le_bytes()).await?;
    writer.write_all(data).await?;
    writer.flush().await
}

fn extract_id(data: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// How long to poll for `bridge.json` after launching DDMM, if it wasn't
/// already running. Overridable via `DDMM_BRIDGE_POLL_TIMEOUT_MS` so the
/// integration test doesn't have to wait 45 real seconds to see
/// `APP_NOT_RUNNING`.
pub fn poll_timeout() -> Duration {
    std::env::var("DDMM_BRIDGE_POLL_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_POLL_TIMEOUT)
}

type AppConnection = (BufReader<OwnedReadHalf>, OwnedWriteHalf);

fn split_connection(stream: TcpStream) -> AppConnection {
    let (read, write) = stream.into_split();
    (BufReader::new(read), write)
}

/// Whether the app side of `conn` is still open, checked without blocking
/// or consuming anything: the app never sends unsolicited data, so a
/// readable socket between requests can only mean EOF (DDMM exited) or an
/// error. `fill_buf` is cancel-safe, so dropping it while pending is fine.
fn connection_alive(conn: &mut AppConnection) -> bool {
    match conn.0.fill_buf().now_or_never() {
        None => true,
        Some(Ok(buf)) => !buf.is_empty(),
        Some(Err(_)) => false,
    }
}

/// Run the host-mode relay to completion (until stdin closes). Never opens
/// a window, never touches Tauri.
///
/// The browser keeps this process (and its port) alive for as long as the
/// extension holds the connection, which can far outlive one DDMM session.
/// So losing the app is not fatal here: the next request reconnects to a
/// fresh `bridge.json` instead of failing with `APP_NOT_RUNNING` because
/// the user closed DDMM at some point since the browser launched the host.
/// If DDMM isn't running at all, only `install`/`open` start it (see
/// `may_launch_app`); everything else gets `APP_NOT_RUNNING` right away.
///
/// `resolve_base` finds the data folder (and so `bridge.json`) again for
/// every (re)connect rather than once: if the user moves DDMM's data folder
/// while the browser keeps this host alive, the restarted app writes its
/// `bridge.json` to the new folder and this finds it there.
pub async fn run(origin: HostOrigin, resolve_base: fn() -> PathBuf) {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    let mut launcher = Launcher::default();
    // Connect to an already-running DDMM only; launching waits for a
    // request that's allowed to (see `may_launch_app`).
    let mut conn = try_connect(&resolve_base()).await.map(split_connection);

    loop {
        let data = match read_frame(&mut stdin, MAX_MESSAGE_BYTES).await {
            Ok(Frame::Eof) | Err(_) => break,
            Ok(Frame::TooLarge) => {
                let reply = ErrorReply::new("", ErrorCode::BadRequest, "message too large").to_line();
                let _ = write_frame(&mut stdout, reply.as_bytes()).await;
                continue;
            }
            Ok(Frame::Data(data)) => data,
        };

        let mut value: serde_json::Value = match serde_json::from_slice(&data) {
            Ok(v) => v,
            Err(_) => {
                let reply = ErrorReply::new("", ErrorCode::BadRequest, "malformed JSON").to_line();
                let _ = write_frame(&mut stdout, reply.as_bytes()).await;
                continue;
            }
        };

        if let Some(obj) = value.as_object_mut() {
            obj.insert("origin".to_string(), serde_json::Value::String(origin.0.clone()));
        }

        let mut line = match serde_json::to_string(&value) {
            Ok(s) => s,
            Err(_) => continue,
        };
        line.push('\n');

        // Only (re)connect *before* sending: a request is never resent
        // after the app may already have received it.
        let request_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let may_launch = may_launch_app(request_type);
        if !conn.as_mut().is_some_and(connection_alive) {
            let stream = if may_launch {
                launcher.ensure_app_running_and_connect(resolve_base, poll_timeout()).await
            } else {
                try_connect(&resolve_base()).await
            };
            conn = stream.map(split_connection);
        }
        let Some((app_reader, app_write)) = conn.as_mut() else {
            let message = if may_launch {
                "Host couldn't start or reach DDMM"
            } else {
                "DDMM isn't running"
            };
            let reply = ErrorReply::new(extract_id(&data), ErrorCode::AppNotRunning, message).to_line();
            let _ = write_frame(&mut stdout, reply.as_bytes()).await;
            continue;
        };

        if app_write.write_all(line.as_bytes()).await.is_err() {
            let reply = ErrorReply::new(extract_id(&data), ErrorCode::AppNotRunning, "lost connection to DDMM").to_line();
            let _ = write_frame(&mut stdout, reply.as_bytes()).await;
            conn = None;
            continue;
        }

        let mut reply_line = String::new();
        match app_reader.read_line(&mut reply_line).await {
            Ok(0) | Err(_) => {
                let reply = ErrorReply::new(extract_id(&data), ErrorCode::AppNotRunning, "lost connection to DDMM").to_line();
                let _ = write_frame(&mut stdout, reply.as_bytes()).await;
                conn = None;
            }
            Ok(_) => {
                let _ = write_frame(&mut stdout, reply_line.trim().as_bytes()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_explicit_user_actions_may_launch_the_app() {
        assert!(may_launch_app("install"));
        assert!(may_launch_app("open"));
        for passive in ["hello", "query", "status", "bogus", ""] {
            assert!(!may_launch_app(passive), "{passive} must not launch DDMM");
        }
    }

    #[test]
    fn detects_chromium_shape() {
        let origin = detect_host_mode(&args(&[
            "/path/to/ddmm",
            "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/",
        ]))
        .unwrap();
        assert_eq!(origin.0, "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/");
    }

    #[test]
    fn detects_chromium_shape_with_windows_parent_window_arg() {
        let origin = detect_host_mode(&args(&[
            "C:\\ddmm.exe",
            "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/",
            "--parent-window=1234",
        ]))
        .unwrap();
        assert_eq!(origin.0, "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/");
    }

    #[test]
    fn detects_firefox_shape() {
        let origin = detect_host_mode(&args(&[
            "/path/to/ddmm",
            "/home/user/.mozilla/native-messaging-hosts/io.github.katsyk.ddmm.json",
            "ddmm@katsyk.github.io",
        ]))
        .unwrap();
        assert_eq!(origin.0, "ddmm@katsyk.github.io");
    }

    #[test]
    fn normal_launch_is_not_host_mode() {
        assert!(detect_host_mode(&args(&["/path/to/ddmm"])).is_none());
        assert!(detect_host_mode(&args(&[])).is_none());
    }

    #[test]
    fn unrelated_single_argument_is_not_host_mode() {
        assert!(detect_host_mode(&args(&["/path/to/ddmm", "--some-flag"])).is_none());
    }

    #[test]
    fn firefox_shape_with_wrong_extension_id_is_not_host_mode() {
        assert!(detect_host_mode(&args(&[
            "/path/to/ddmm",
            "/path/to/manifest.json",
            "not-our-extension@example.com",
        ]))
        .is_none());
    }

    #[tokio::test]
    async fn frame_round_trips() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        write_frame(&mut client, b"{\"id\":\"1\"}").await.unwrap();
        drop(client);

        let frame = read_frame(&mut server, MAX_MESSAGE_BYTES).await.unwrap();
        match frame {
            Frame::Data(data) => assert_eq!(data, b"{\"id\":\"1\"}"),
            _ => panic!("expected data frame"),
        }
    }

    #[tokio::test]
    async fn oversized_frame_is_drained_and_reported() {
        // Bigger than everything written below -- `duplex`'s buffer is
        // bounded, and both writes happen before anything reads, so an
        // undersized buffer would deadlock (`write_all` awaiting capacity
        // that only a concurrent reader would ever free).
        let (mut client, mut server) = tokio::io::duplex(4 * 1024 * 1024);
        let big = vec![b'x'; (MAX_MESSAGE_BYTES + 10) as usize];
        write_frame(&mut client, &big).await.unwrap();
        // A normal frame right after -- proves the stream stayed in sync.
        write_frame(&mut client, b"{\"id\":\"ok\"}").await.unwrap();
        drop(client);

        let first = read_frame(&mut server, MAX_MESSAGE_BYTES).await.unwrap();
        assert!(matches!(first, Frame::TooLarge));

        let second = read_frame(&mut server, MAX_MESSAGE_BYTES).await.unwrap();
        match second {
            Frame::Data(data) => assert_eq!(data, b"{\"id\":\"ok\"}"),
            _ => panic!("expected data frame after the oversized one"),
        }
    }

    #[tokio::test]
    async fn empty_stream_is_eof() {
        let (client, mut server) = tokio::io::duplex(64);
        drop(client);
        let frame = read_frame(&mut server, MAX_MESSAGE_BYTES).await.unwrap();
        assert!(matches!(frame, Frame::Eof));
    }

    #[test]
    fn extract_id_reads_id_field() {
        assert_eq!(extract_id(br#"{"id":"42","type":"hello"}"#), "42");
    }

    #[test]
    fn extract_id_defaults_to_empty_for_missing_or_malformed() {
        assert_eq!(extract_id(b"not json"), "");
        assert_eq!(extract_id(br#"{"type":"hello"}"#), "");
    }

    /// `poll_timeout` reads a process-global env var; cargo runs tests in
    /// parallel threads of the same process, so the two tests below must
    /// never interleave their set/remove with each other (or they'll
    /// observe each other's value and flake).
    static POLL_TIMEOUT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn poll_timeout_defaults_to_45s_without_env_var() {
        let _guard = POLL_TIMEOUT_ENV_LOCK.lock().unwrap();
        // SAFETY: serialized against the other env-mutating test above by
        // POLL_TIMEOUT_ENV_LOCK; no other test in this crate touches this
        // var.
        unsafe { std::env::remove_var("DDMM_BRIDGE_POLL_TIMEOUT_MS") };
        assert_eq!(poll_timeout(), Duration::from_secs(45));
    }

    #[test]
    fn poll_timeout_honors_env_override() {
        let _guard = POLL_TIMEOUT_ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("DDMM_BRIDGE_POLL_TIMEOUT_MS", "250") };
        assert_eq!(poll_timeout(), Duration::from_millis(250));
        unsafe { std::env::remove_var("DDMM_BRIDGE_POLL_TIMEOUT_MS") };
    }
}
