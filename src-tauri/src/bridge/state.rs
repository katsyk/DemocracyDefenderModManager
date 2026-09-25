//! `bridge.json`: how the native-messaging host (a separate process,
//! possibly launched fresh) finds and authenticates to the already-running
//! DDMM app.

use std::path::{Path, PathBuf};

use rand::{rngs::SysRng, TryRng};
use serde::{Deserialize, Serialize};

use crate::bridge::protocol::PROTOCOL_VERSION;

pub const BRIDGE_FILE_NAME: &str = "bridge.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub protocol: u32,
}

impl BridgeInfo {
    pub fn new(port: u16, token: String) -> Self {
        Self {
            port,
            token,
            pid: std::process::id(),
            protocol: PROTOCOL_VERSION,
        }
    }
}

pub fn bridge_file_path(base_path: &Path) -> PathBuf {
    base_path.join(BRIDGE_FILE_NAME)
}

/// A fresh, 256-bit, hex-encoded token. Regenerated on every app start --
/// see the security checklist in the protocol spec.
pub fn generate_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    SysRng.try_fill_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time comparison of the token the caller supplied against the
/// one in `bridge.json`. A plain `==` would let a timing attack narrow the
/// token byte by byte; this always inspects every byte of both strings
/// regardless of where they first differ.
pub fn tokens_match(expected: &str, supplied: &str) -> bool {
    let expected = expected.as_bytes();
    let supplied = supplied.as_bytes();

    // Length itself isn't secret (the token is a fixed-length hex string),
    // but bail out without a variable-time byte loop over mismatched
    // lengths.
    if expected.len() != supplied.len() {
        return false;
    }

    let mut diff: u8 = 0;
    for (a, b) in expected.iter().zip(supplied.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Write `bridge.json` with owner-only permissions: `0600` on Unix. On
/// Windows there's no direct equivalent of Unix mode bits, but the file
/// already lives under the per-user app-data directory (or the portable
/// folder, which is whatever the user already controls), which is not
/// group/world-readable by default on a normal Windows install -- so no
/// extra ACL call is made there; see the protocol spec's note on this.
pub async fn write_bridge_file(base_path: &Path, info: &BridgeInfo) -> anyhow::Result<()> {
    let path = bridge_file_path(base_path);
    let data = serde_json::to_vec_pretty(info)?;
    tokio::fs::write(&path, data).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&path, perms).await?;
    }

    Ok(())
}

pub async fn read_bridge_file(base_path: &Path) -> anyhow::Result<BridgeInfo> {
    let path = bridge_file_path(base_path);
    let data = tokio::fs::read(&path).await?;
    Ok(serde_json::from_slice(&data)?)
}

pub async fn remove_bridge_file(base_path: &Path) {
    let _ = tokio::fs::remove_file(bridge_file_path(base_path)).await;
}

/// Best-effort check for whether `pid` still refers to a live process.
/// Only used to decide whether a stale `bridge.json` is worth trying to
/// connect to at all before falling back to "start DDMM" -- a false
/// positive here just costs one failed connection attempt, not a security
/// decision.
#[cfg(unix)]
pub fn process_is_alive(pid: u32) -> bool {
    // Signal 0: no signal sent, just existence/permission checked.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(windows)]
pub fn process_is_alive(pid: u32) -> bool {
    use std::process::Command;
    // No extra crate: ask `tasklist` to filter on this exact PID and see
    // whether it printed a matching line.
    let Ok(output) = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
    else {
        return true; // Can't tell; don't block on it.
    };
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_64_hex_chars_and_differ() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn tokens_match_identical() {
        assert!(tokens_match("abc123", "abc123"));
    }

    #[test]
    fn tokens_match_rejects_different_value() {
        assert!(!tokens_match("abc123", "abc124"));
    }

    #[test]
    fn tokens_match_rejects_different_length() {
        assert!(!tokens_match("abc123", "abc12"));
        assert!(!tokens_match("abc123", "abc1234"));
    }

    #[tokio::test]
    async fn bridge_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let info = BridgeInfo::new(51234, "deadbeef".to_string());

        write_bridge_file(dir.path(), &info).await.unwrap();
        let loaded = read_bridge_file(dir.path()).await.unwrap();

        assert_eq!(loaded.port, 51234);
        assert_eq!(loaded.token, "deadbeef");
        assert_eq!(loaded.protocol, PROTOCOL_VERSION);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bridge_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let info = BridgeInfo::new(51234, "deadbeef".to_string());
        write_bridge_file(dir.path(), &info).await.unwrap();

        let meta = std::fs::metadata(bridge_file_path(dir.path())).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn remove_bridge_file_deletes_it() {
        let dir = tempfile::tempdir().unwrap();
        let info = BridgeInfo::new(1, "x".to_string());
        write_bridge_file(dir.path(), &info).await.unwrap();
        assert!(bridge_file_path(dir.path()).exists());

        remove_bridge_file(dir.path()).await;
        assert!(!bridge_file_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn read_missing_bridge_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_bridge_file(dir.path()).await.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn process_is_alive_true_for_self() {
        assert!(process_is_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn process_is_alive_false_for_bogus_pid() {
        // PID 1 is always init/systemd (alive); use a PID far outside any
        // realistic range instead.
        assert!(!process_is_alive(u32::MAX - 1));
    }
}
