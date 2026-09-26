//! Storage for the secrets DDMM can hold, both optional and both only used
//! for Nexus Mods update checks:
//! - the user's personal Nexus Mods API key (entered manually), and
//! - the Nexus Mods sign-in tokens from "Sign in to Nexus Mods" (OAuth;
//!   see `crate::nexus_oauth`).
//!
//! Each lives in the OS keychain (Windows Credential Manager, macOS
//! Keychain, Linux Secret Service) via the `keyring` crate. On Linux
//! without a Secret Service provider (no GNOME Keyring/KWallet running, a
//! bare window manager, ...) it falls back to a file in DDMM's data folder
//! with owner-only (0600) permissions, and the UI says so.
//!
//! Rules this module exists to enforce:
//! - secrets are never written to `settings.json` or any other settings file;
//! - they are never logged -- [`NexusApiKey`]'s and [`Secret`]'s `Debug`
//!   print `[redacted]`, and [`redact`]/[`redact_all`] scrub them out of any
//!   error text before that text can reach a log line or the UI;
//! - they are only ever sent to Nexus Mods (`api.nexusmods.com`, plus
//!   `users.nexusmods.com` for the sign-in tokens -- enforced in
//!   `providers::nexus` and `nexus_oauth`), and never cross the browser
//!   bridge or reach the frontend.

use std::path::{Path, PathBuf};

use serde::Serialize;

const KEYRING_SERVICE: &str = "io.github.katsyk.ddmm";

/// One stored secret: its keychain entry name and its Linux-only fallback
/// file (in DDMM's data folder, used when no Secret Service provider is
/// available).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    keyring_user: &'static str,
    pub(crate) file_name: &'static str,
}

/// The personal API key.
pub const API_KEY_SLOT: Slot = Slot { keyring_user: "nexus-api-key", file_name: FALLBACK_FILE_NAME };
/// The Nexus Mods sign-in (OAuth) tokens, as one JSON document.
pub const OAUTH_SLOT: Slot = Slot { keyring_user: "nexus-oauth-tokens", file_name: "nexus-oauth-tokens" };
/// Fallback file for the API key.
pub const FALLBACK_FILE_NAME: &str = "nexus-api-key";

/// Windows Credential Manager caps one credential at 2560 bytes, and
/// `keyring` stores passwords there as UTF-16 -- so at most 1280
/// characters. Sign-in tokens (JWTs) can come close, so anything longer
/// than this is split across several entries (`<name>#0`, `<name>#1`, ...)
/// with a small header in `<name>` itself. Short values (the API key) are
/// stored as-is, exactly as before.
const KEYCHAIN_CHUNK: usize = 1000;
const CHUNK_HEADER: &str = "ddmm-chunked:";
/// Upper bound on chunks we'll read back (sanity check on the header).
const MAX_CHUNKS: usize = 64;

/// Where a stored key actually lives -- shown in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum KeyStorage {
    /// The OS keychain / credential store.
    Keychain,
    /// `FALLBACK_FILE_NAME` in the data folder, mode 0600 (Linux only).
    File,
}

/// A Nexus API key. Deliberately has no `Display`, and a `Debug` that never
/// prints the value, so it can't end up in a log line by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct NexusApiKey(String);

impl NexusApiKey {
    /// Trim and sanity-check a key the user pasted. This is only a shape
    /// check (Nexus itself decides whether the key is valid); it exists so
    /// obvious junk (a URL, a multi-line paste) is rejected with a clear
    /// message and never sent anywhere.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let key = raw.trim();
        if key.is_empty() {
            return Err("the key is empty");
        }
        if key.len() > 1024 {
            return Err("that's too long to be a Nexus Mods API key");
        }
        if !key.chars().all(|c| c.is_ascii_graphic()) {
            return Err("the key contains spaces or characters a Nexus Mods API key never has");
        }
        Ok(Self(key.to_string()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for NexusApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NexusApiKey([redacted])")
    }
}

/// Any other secret string (sign-in tokens). Like [`NexusApiKey`]: no
/// `Display`, and a `Debug` that never prints the value.
#[derive(Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Replace every occurrence of each of `secrets` in `text` with
/// `[redacted]` (empty strings are ignored).
pub fn redact_all(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            out = out.replace(secret, "[redacted]");
        }
    }
    out
}

/// Replace every occurrence of `key` in `text` with `[redacted]`. Applied to
/// every error message that could possibly have been built from a request
/// that carried the key.
pub fn redact(text: &str, key: &NexusApiKey) -> String {
    if key.0.is_empty() {
        return text.to_string();
    }
    text.replace(&key.0, "[redacted]")
}

#[cfg(test)]
fn fallback_path(base_path: &Path) -> PathBuf {
    slot_file(base_path, API_KEY_SLOT)
}

fn slot_file(base_path: &Path, slot: Slot) -> PathBuf {
    base_path.join(slot.file_name)
}

/// Unit tests must never touch the real keychain of whoever runs them (it
/// would overwrite their actual DDMM secrets), so under `cfg(test)` storage
/// goes to the fallback file unless a test opts in with
/// `DDMM_TEST_REAL_KEYCHAIN` (the `#[ignore]`d keychain tests do).
fn keychain_enabled() -> bool {
    !cfg!(test) || std::env::var_os("DDMM_TEST_REAL_KEYCHAIN").is_some()
}

fn keyring_entry(user: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, user)
}

fn chunk_user(slot: Slot, i: usize) -> String {
    format!("{}#{i}", slot.keyring_user)
}

/// Split `secret` into keychain-sized pieces (on char boundaries).
fn split_chunks(secret: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut rest = secret;
    while !rest.is_empty() {
        let mut end = rest.len().min(KEYCHAIN_CHUNK);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&rest[..end]);
        rest = &rest[end..];
    }
    chunks
}

/// Blocking: write `secret` to the keychain (chunked if long) and read it
/// back.
fn keychain_write(slot: Slot, secret: &str) -> keyring::Result<()> {
    // Drop whatever was there before (including leftover chunks of a longer
    // older value).
    keychain_delete(slot)?;
    let head = keyring_entry(slot.keyring_user)?;
    if secret.len() <= KEYCHAIN_CHUNK && !secret.starts_with(CHUNK_HEADER) {
        head.set_password(secret)?;
    } else {
        let chunks = split_chunks(secret);
        for (i, chunk) in chunks.iter().enumerate() {
            keyring_entry(&chunk_user(slot, i))?.set_password(chunk)?;
        }
        head.set_password(&format!("{CHUNK_HEADER}{}", chunks.len()))?;
    }
    // Read it back: some headless setups accept the write and then can't
    // return it, which would look like "saved" but never work.
    match keychain_read(slot) {
        Ok(read) if read == secret => Ok(()),
        Ok(_) => Err(keyring::Error::PlatformFailure("stored value didn't read back".into())),
        Err(e) => Err(e),
    }
}

/// Blocking: read a (possibly chunked) value from the keychain.
fn keychain_read(slot: Slot) -> keyring::Result<String> {
    let head = keyring_entry(slot.keyring_user)?.get_password()?;
    let Some(count) = head.strip_prefix(CHUNK_HEADER) else {
        return Ok(head);
    };
    let count: usize = count
        .parse()
        .ok()
        .filter(|n| (1..=MAX_CHUNKS).contains(n))
        .ok_or_else(|| keyring::Error::PlatformFailure("damaged keychain entry".into()))?;
    let mut out = String::new();
    for i in 0..count {
        out.push_str(&keyring_entry(&chunk_user(slot, i))?.get_password()?);
    }
    Ok(out)
}

/// Blocking: remove a (possibly chunked) value. Missing entries are fine.
fn keychain_delete(slot: Slot) -> keyring::Result<()> {
    let ignore_missing = |r: keyring::Result<()>| match r {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    };
    for i in 0..MAX_CHUNKS {
        match keyring_entry(&chunk_user(slot, i))?.delete_credential() {
            Ok(()) => {}
            Err(keyring::Error::NoEntry) => break,
            Err(e) => return Err(e),
        }
    }
    ignore_missing(keyring_entry(slot.keyring_user)?.delete_credential())
}

/// Store `key`, preferring the OS keychain. Returns where it ended up.
pub async fn store(base_path: &Path, key: &NexusApiKey) -> anyhow::Result<KeyStorage> {
    store_secret(base_path, API_KEY_SLOT, key.expose())
        .await
        .map_err(|e| anyhow::anyhow!(redact(&e.to_string(), key)))
}

/// Store any secret string in `slot`, preferring the OS keychain. Returns
/// where it ended up. Errors never contain the secret.
pub async fn store_secret(base_path: &Path, slot: Slot, secret: &str) -> anyhow::Result<KeyStorage> {
    if !keychain_enabled() {
        write_slot_file(base_path, slot, secret).await?;
        return Ok(KeyStorage::File);
    }
    let value = secret.to_string();
    let keychain = tokio::task::spawn_blocking(move || keychain_write(slot, &value)).await?;

    match keychain {
        Ok(()) => {
            // A secret saved to the keychain supersedes any older fallback file.
            remove_slot_file(base_path, slot).await;
            Ok(KeyStorage::Keychain)
        }
        Err(e) => {
            // `keyring` errors never contain the secret itself, but scrub
            // anyway -- this text goes to the log.
            let reason = redact_all(&e.to_string(), &[secret]);
            if cfg!(target_os = "linux") {
                log::warn!(
                    "OS keychain (Secret Service) unavailable ({reason}); storing {} in an owner-only file in the data folder instead.",
                    slot.file_name
                );
                write_slot_file(base_path, slot, secret).await?;
                Ok(KeyStorage::File)
            } else {
                anyhow::bail!("couldn't save to the system keychain: {reason}")
            }
        }
    }
}

/// Load the stored key, if any: the fallback file first (it only exists
/// when the keychain wasn't usable at save time), then the keychain.
pub async fn load(base_path: &Path) -> Option<(NexusApiKey, KeyStorage)> {
    let (raw, storage) = load_secret(base_path, API_KEY_SLOT).await?;
    NexusApiKey::parse(&raw).ok().map(|k| (k, storage))
}

/// Load the raw secret in `slot`, if any: the fallback file first, then
/// the keychain.
pub async fn load_secret(base_path: &Path, slot: Slot) -> Option<(String, KeyStorage)> {
    if let Some(value) = read_slot_file(base_path, slot).await {
        return Some((value, KeyStorage::File));
    }
    if !keychain_enabled() {
        return None;
    }

    let loaded = tokio::task::spawn_blocking(move || keychain_read(slot)).await.ok()?;
    match loaded {
        Ok(secret) if !secret.trim().is_empty() => Some((secret, KeyStorage::Keychain)),
        Ok(_) | Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            log::debug!("Nothing available from the OS keychain for {}: {e}", slot.keyring_user);
            None
        }
    }
}

/// Remove the key from everywhere it could be stored. Never fails: a key
/// that wasn't there is already removed.
pub async fn remove(base_path: &Path) {
    remove_secret(base_path, API_KEY_SLOT).await
}

/// Remove the secret in `slot` from everywhere it could be stored.
pub async fn remove_secret(base_path: &Path, slot: Slot) {
    remove_slot_file(base_path, slot).await;
    if !keychain_enabled() {
        return;
    }
    let result = tokio::task::spawn_blocking(move || keychain_delete(slot)).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::debug!("Removing {} from the OS keychain: {e}", slot.keyring_user),
        Err(e) => log::debug!("Removing {} from the OS keychain: {e}", slot.keyring_user),
    }
}

#[cfg(test)]
async fn write_fallback_file(base_path: &Path, key: &NexusApiKey) -> anyhow::Result<()> {
    write_slot_file(base_path, API_KEY_SLOT, key.expose()).await
}

async fn write_slot_file(base_path: &Path, slot: Slot, secret: &str) -> anyhow::Result<()> {
    let path = slot_file(base_path, slot);
    // Create with 0600 from the start (not write-then-chmod), so the secret
    // is never briefly readable by other users.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let secret = secret.to_string();
        let path2 = path.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path2)?;
            // An existing file keeps its old mode on open; force it.
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            f.write_all(secret.as_bytes())?;
            f.sync_all()
        })
        .await??;
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(&path, secret.as_bytes()).await?;
    }
    Ok(())
}

#[cfg(test)]
async fn read_fallback_file(base_path: &Path) -> Option<NexusApiKey> {
    let data = read_slot_file(base_path, API_KEY_SLOT).await?;
    NexusApiKey::parse(&data).ok()
}

async fn read_slot_file(base_path: &Path, slot: Slot) -> Option<String> {
    let data = tokio::fs::read_to_string(slot_file(base_path, slot)).await.ok()?;
    let data = data.trim();
    (!data.is_empty()).then(|| data.to_string())
}

#[cfg(test)]
async fn remove_fallback_file(base_path: &Path) {
    remove_slot_file(base_path, API_KEY_SLOT).await
}

async fn remove_slot_file(base_path: &Path, slot: Slot) {
    let _ = tokio::fs::remove_file(slot_file(base_path, slot)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_trims_and_rejects_junk() {
        assert_eq!(NexusApiKey::parse("  abc123==  \n").unwrap().expose(), "abc123==");
        assert!(NexusApiKey::parse("").is_err());
        assert!(NexusApiKey::parse("   ").is_err());
        assert!(NexusApiKey::parse("two words").is_err());
        assert!(NexusApiKey::parse("line\nbreak").is_err());
        assert!(NexusApiKey::parse(&"x".repeat(2000)).is_err());
    }

    #[test]
    fn debug_never_prints_the_key() {
        let key = NexusApiKey::parse("SuperSecretKey123").unwrap();
        let printed = format!("{key:?} {:?}", Some(&key));
        assert!(!printed.contains("SuperSecretKey123"), "{printed}");
    }

    #[test]
    fn redact_scrubs_every_occurrence() {
        let key = NexusApiKey::parse("SuperSecretKey123").unwrap();
        let text = "bad header apikey=SuperSecretKey123 (SuperSecretKey123)";
        let out = redact(text, &key);
        assert!(!out.contains("SuperSecretKey123"));
        assert_eq!(out.matches("[redacted]").count(), 2);
    }

    /// Round trip through the real OS keychain. `#[ignore]`d: it needs a
    /// keychain and writes to it. On Linux, run it in a throwaway session so
    /// it never touches your real keyring:
    /// `dbus-run-session -- sh -c 'echo -n x | gnome-keyring-daemon --unlock --components=secrets >/dev/null; cargo test -- --ignored --test-threads=1 os_keychain'`
    #[tokio::test]
    #[ignore]
    async fn os_keychain_round_trip() {
        std::env::set_var("DDMM_TEST_REAL_KEYCHAIN", "1");
        let dir = tempfile::tempdir().unwrap();
        let key = NexusApiKey::parse("KeychainTestKey789").unwrap();
        assert_eq!(store(dir.path(), &key).await.unwrap(), KeyStorage::Keychain);
        assert!(!fallback_path(dir.path()).exists(), "keychain storage must not also write the file");
        assert_eq!(load(dir.path()).await, Some((key, KeyStorage::Keychain)));
        remove(dir.path()).await;
        assert_eq!(load(dir.path()).await, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_file_is_owner_only_and_round_trips() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key = NexusApiKey::parse("FileKey456").unwrap();
        write_fallback_file(dir.path(), &key).await.unwrap();

        let meta = std::fs::metadata(fallback_path(dir.path())).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(read_fallback_file(dir.path()).await, Some(key));

        remove_fallback_file(dir.path()).await;
        assert!(read_fallback_file(dir.path()).await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_file_tightens_an_existing_loose_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = fallback_path(dir.path());
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_fallback_file(dir.path(), &NexusApiKey::parse("NewKey").unwrap()).await.unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn secret_debug_is_redacted_and_redact_all_scrubs_each() {
        let a = Secret::new("AccessTokenABC");
        assert!(!format!("{a:?}").contains("AccessTokenABC"));
        let out = redact_all("x AccessTokenABC y RefreshXYZ z", &[a.expose(), "RefreshXYZ", ""]);
        assert_eq!(out, "x [redacted] y [redacted] z");
    }

    #[test]
    fn long_values_split_into_keychain_sized_chunks() {
        let long = "a".repeat(KEYCHAIN_CHUNK * 2 + 17);
        let chunks = split_chunks(&long);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= KEYCHAIN_CHUNK));
        // UTF-16 on Windows: 2 bytes per char must fit the 2560-byte cap.
        assert!(KEYCHAIN_CHUNK * 2 <= 2560);
        assert_eq!(chunks.concat(), long);
        assert_eq!(split_chunks("short"), vec!["short"]);
    }

    /// Chunked round trip through the real OS keychain; `#[ignore]`d like
    /// [`os_keychain_round_trip`].
    #[tokio::test]
    #[ignore]
    async fn os_keychain_round_trip_long_tokens() {
        std::env::set_var("DDMM_TEST_REAL_KEYCHAIN", "1");
        let dir = tempfile::tempdir().unwrap();
        let long = format!("{{\"access_token\":\"{}\"}}", "t".repeat(3000));
        assert_eq!(store_secret(dir.path(), OAUTH_SLOT, &long).await.unwrap(), KeyStorage::Keychain);
        assert_eq!(load_secret(dir.path(), OAUTH_SLOT).await, Some((long, KeyStorage::Keychain)));
        store_secret(dir.path(), OAUTH_SLOT, "short-now").await.unwrap();
        assert_eq!(load_secret(dir.path(), OAUTH_SLOT).await.unwrap().0, "short-now");
        remove_secret(dir.path(), OAUTH_SLOT).await;
        assert_eq!(load_secret(dir.path(), OAUTH_SLOT).await, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oauth_fallback_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_slot_file(dir.path(), OAUTH_SLOT, "{\"access_token\":\"x\"}").await.unwrap();
        let meta = std::fs::metadata(slot_file(dir.path(), OAUTH_SLOT)).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(read_slot_file(dir.path(), OAUTH_SLOT).await.as_deref(), Some("{\"access_token\":\"x\"}"));
        // Separate from the API key's file.
        assert!(read_fallback_file(dir.path()).await.is_none());
    }
}
