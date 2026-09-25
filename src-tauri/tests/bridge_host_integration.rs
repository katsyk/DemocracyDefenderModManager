//! Spawns the actual built `ddmm` binary in native-messaging host mode and
//! exercises it as a real browser would: framed stdin/stdout, the
//! `bridge.json` + token handshake against a fake "app" TCP server, origin
//! injection, oversized-frame rejection, and the `APP_NOT_RUNNING` timeout
//! when nothing is listening. See `docs/development/bridge-protocol.md`.

use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

const CHROME_EXTENSION_ARG: &str = "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/";

/// Copy the compiled `ddmm` binary into a fresh portable-mode temp
/// directory (a `portable.txt` marker makes `data_dir::decide_base_dir`
/// resolve the base path to this same directory, deterministically, with
/// no dependence on the host machine's real app-data location).
fn portable_copy_of_binary() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("portable.txt"), "").unwrap();

    let src = PathBuf::from(env!("CARGO_BIN_EXE_ddmm"));
    let dest = dir.path().join(src.file_name().unwrap());
    std::fs::copy(&src, &dest).unwrap();
    // Belt and braces against the ETXTBSY flake `spawn_host` also retries
    // around: make sure the copy is fully flushed to the filesystem (and
    // the handle closed) before anything tries to exec it.
    std::fs::File::open(&dest).unwrap().sync_all().unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).unwrap();
    }

    (dir, dest)
}

/// A no-op executable script, written into `dir`, that exits immediately.
/// Used as a stand-in for "launch DDMM" so the timeout test never actually
/// starts a full desktop app (which would need a display).
#[cfg(unix)]
fn dummy_launch_target(dir: &Path) -> PathBuf {
    let path = dir.join("dummy-launch.sh");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn write_frame(stream: &mut impl Write, data: &[u8]) {
    stream.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    stream.write_all(data).unwrap();
    stream.flush().unwrap();
}

fn read_frame(stream: &mut impl Read) -> Vec<u8> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).unwrap();
    buf
}

fn read_frame_json(stream: &mut impl Read) -> serde_json::Value {
    serde_json::from_slice(&read_frame(stream)).unwrap()
}

fn spawn_host(exe: &Path, extra_env: &[(&str, &str)]) -> Child {
    // `fs::copy` (in `portable_copy_of_binary`) returns before the kernel
    // has necessarily finished releasing the destination inode for exec on
    // every filesystem, so an exec right after a fresh copy can transiently
    // fail with ETXTBSY ("Text file busy") -- a handful of short retries
    // clears it reliably without weakening what the test actually checks.
    let mut last_err = None;
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(50));
        }

        let mut cmd = Command::new(exe);
        cmd.arg(CHROME_EXTENSION_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        match cmd.spawn() {
            Ok(child) => return child,
            Err(e) if e.raw_os_error() == Some(26) /* ETXTBSY */ => last_err = Some(e),
            Err(e) => panic!("failed to spawn ddmm in host mode: {e}"),
        }
    }
    panic!("failed to spawn ddmm in host mode after retries: {:?}", last_err.unwrap())
}

/// A minimal stand-in for `bridge::server`: accepts one connection,
/// performs the `{"hello": token}` handshake, then echoes back a crafted
/// `hello` reply for the first line it reads and records every line it
/// receives so the test can assert on origin injection.
fn spawn_fake_app_server(expected_token: &str) -> (u16, std::sync::mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let expected_token = expected_token.to_string();
    let (tx, rx) = std::sync::mpsc::channel();

    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        use std::io::BufRead;
        let mut handshake = String::new();
        reader.read_line(&mut handshake).unwrap();
        let handshake: serde_json::Value = serde_json::from_str(handshake.trim()).unwrap();
        assert_eq!(handshake["hello"], expected_token);
        stream.write_all(b"{\"ok\":true}\n").unwrap();

        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let _ = tx.send(trimmed.to_string());

            let req: serde_json::Value = serde_json::from_str(trimmed).unwrap();
            let id = req["id"].as_str().unwrap_or("");
            let reply = serde_json::json!({
                "id": id, "ok": true, "type": "hello",
                "appVersion": "2.0.0-rc.3", "protocol": 1,
                "afterInstall": "deploy", "gameFound": false
            });
            stream.write_all(reply.to_string().as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
        }
    });

    (port, rx, handle)
}

fn write_bridge_json(dir: &Path, port: u16, token: &str) {
    let content = serde_json::json!({ "port": port, "token": token, "pid": 999999u32, "protocol": 1 });
    std::fs::write(dir.join("bridge.json"), content.to_string()).unwrap();
}

#[test]
fn relays_hello_and_injects_origin() {
    let (dir, exe) = portable_copy_of_binary();
    let token = "a".repeat(64);
    let (port, received, _server) = spawn_fake_app_server(&token);
    write_bridge_json(dir.path(), port, &token);

    let mut child = spawn_host(&exe, &[]);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    write_frame(&mut stdin, br#"{"id":"1","type":"hello"}"#);
    let reply = read_frame_json(&mut stdout);
    assert_eq!(reply["id"], "1");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["type"], "hello");

    let forwarded = received.recv_timeout(Duration::from_secs(5)).unwrap();
    let forwarded: serde_json::Value = serde_json::from_str(&forwarded).unwrap();
    assert_eq!(forwarded["origin"], CHROME_EXTENSION_ARG);
    assert_eq!(forwarded["type"], "hello");

    drop(stdin);
    let _ = child.wait_timeout_or_kill();
}

#[test]
fn oversized_frame_is_rejected_without_desyncing_the_stream() {
    let (dir, exe) = portable_copy_of_binary();
    let token = "b".repeat(64);
    let (port, _received, _server) = spawn_fake_app_server(&token);
    write_bridge_json(dir.path(), port, &token);

    let mut child = spawn_host(&exe, &[]);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let big = vec![b'x'; 1024 * 1024 + 10];
    write_frame(&mut stdin, &big);

    let reply = read_frame_json(&mut stdout);
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "BAD_REQUEST");

    // The stream must still be in sync afterward.
    write_frame(&mut stdin, br#"{"id":"2","type":"hello"}"#);
    let reply2 = read_frame_json(&mut stdout);
    assert_eq!(reply2["id"], "2");
    assert_eq!(reply2["ok"], true);

    drop(stdin);
    let _ = child.wait_timeout_or_kill();
}

#[test]
#[cfg(unix)]
fn app_not_running_replies_with_error_after_the_configured_timeout() {
    let (dir, exe) = portable_copy_of_binary();
    // No bridge.json is written -- the host must fail to connect, "launch"
    // (the dummy, so no real desktop app starts), poll, time out, and
    // reply APP_NOT_RUNNING to every request it's holding.
    let dummy = dummy_launch_target(dir.path());

    let started = std::time::Instant::now();
    let mut child = spawn_host(
        &exe,
        &[
            ("APPIMAGE", dummy.to_str().unwrap()),
            ("DDMM_BRIDGE_POLL_TIMEOUT_MS", "300"),
        ],
    );
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    write_frame(&mut stdin, br#"{"id":"1","type":"hello"}"#);
    let reply = read_frame_json(&mut stdout);
    let elapsed = started.elapsed();

    assert_eq!(reply["id"], "1");
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "APP_NOT_RUNNING");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}, expected the short configured timeout to apply");

    drop(stdin);
    let _ = child.wait_timeout_or_kill();
}

/// Children of `pid` that have exited but were never waited for.
#[cfg(target_os = "linux")]
fn zombie_children_of(pid: u32) -> Vec<u32> {
    let mut zombies = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(child_pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        // "<pid> (<comm>) <state> <ppid> ..." -- comm may contain spaces,
        // so split after the last ')'.
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { continue };
        let mut fields = rest.split_whitespace();
        let state = fields.next();
        let ppid = fields.next().and_then(|p| p.parse::<u32>().ok());
        if ppid == Some(pid) && state == Some("Z") {
            zombies.push(child_pid);
        }
    }
    zombies
}

#[test]
#[cfg(target_os = "linux")]
fn launched_app_is_reaped_when_it_exits() {
    let (dir, exe) = portable_copy_of_binary();
    // The dummy "DDMM" exits immediately, like a real one the user closed
    // while the browser keeps the host running.
    let dummy = dummy_launch_target(dir.path());

    let mut child = spawn_host(
        &exe,
        &[
            ("APPIMAGE", dummy.to_str().unwrap()),
            ("DDMM_BRIDGE_POLL_TIMEOUT_MS", "300"),
        ],
    );
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    write_frame(&mut stdin, br#"{"id":"1","type":"hello"}"#);
    let reply = read_frame_json(&mut stdout);
    assert_eq!(reply["error"]["code"], "APP_NOT_RUNNING");

    // The host is still alive (stdin open); its launched child must not be
    // left as a zombie.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(zombie_children_of(child.id()), Vec::<u32>::new());

    drop(stdin);
    let _ = child.wait_timeout_or_kill();
}

/// A fake "app" that completes the handshake, answers exactly one request
/// (echoing its id with `served_by`), then closes the connection -- i.e.
/// DDMM exiting while the browser keeps the host alive.
fn spawn_one_shot_app_server(token: &str, served_by: &'static str) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = token.to_string();
    let handle = std::thread::spawn(move || {
        use std::io::BufRead;
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let handshake: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(handshake["hello"], token);
        stream.write_all(b"{\"ok\":true}\n").unwrap();

        line.clear();
        reader.read_line(&mut line).unwrap();
        let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let reply = serde_json::json!({ "id": req["id"], "ok": true, "type": "status", "servedBy": served_by });
        stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
        // Dropping `stream` (and the listener) here closes everything.
    });
    (port, handle)
}

#[test]
#[cfg(unix)]
fn reconnects_after_the_app_restarts_instead_of_failing() {
    let (dir, exe) = portable_copy_of_binary();
    let dummy = dummy_launch_target(dir.path());

    let (port1, first) = spawn_one_shot_app_server("token-one", "first");
    write_bridge_json(dir.path(), port1, "token-one");

    let mut child = spawn_host(
        &exe,
        &[
            ("APPIMAGE", dummy.to_str().unwrap()),
            ("DDMM_BRIDGE_POLL_TIMEOUT_MS", "3000"),
        ],
    );
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    write_frame(&mut stdin, br#"{"id":"1","type":"status"}"#);
    let reply = read_frame_json(&mut stdout);
    assert_eq!(reply["id"], "1");
    assert_eq!(reply["servedBy"], "first");
    first.join().unwrap();

    // "DDMM" exited and a new instance came up with a new port and token.
    let (port2, second) = spawn_one_shot_app_server("token-two", "second");
    write_bridge_json(dir.path(), port2, "token-two");
    std::thread::sleep(Duration::from_millis(200));

    write_frame(&mut stdin, br#"{"id":"2","type":"status"}"#);
    let reply = read_frame_json(&mut stdout);
    assert_eq!(reply["id"], "2");
    assert_eq!(reply["ok"], true, "expected a reconnect, got {reply}");
    assert_eq!(reply["servedBy"], "second");
    second.join().unwrap();

    drop(stdin);
    let _ = child.wait_timeout_or_kill();
}

/// Small helper so the tests above don't hang forever if something went
/// wrong: give the child a moment to exit on its own (stdin closed), then
/// kill it.
trait ChildExt {
    fn wait_timeout_or_kill(&mut self) -> std::io::Result<()>;
}

impl ChildExt for Child {
    fn wait_timeout_or_kill(&mut self) -> std::io::Result<()> {
        for _ in 0..50 {
            if self.try_wait()?.is_some() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.kill()
    }
}
