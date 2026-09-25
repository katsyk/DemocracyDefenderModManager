//! Storage for the one secret DDMM can hold: the user's optional, personal
//! Nexus Mods API key (used only for update checks -- see
//! `providers::nexus`).
//!
//! The key lives in the OS keychain (Windows Credential Manager, macOS
//! Keychain, Linux Secret Service) via the `keyring` crate. On Linux
//! without a Secret Service provider (no GNOME Keyring/KWallet running, a
//! bare window manager, ...) it falls back to a file in DDMM's data folder
//! with owner-only (0600) permissions, and the UI says so.
//!
//! Rules this module exists to enforce:
//! - the key is never written to `settings.json` or any other settings file;
//! - it is never logged -- [`NexusApiKey`]'s `Debug` prints `[redacted]`, and
//!   [`redact`] scrubs it out of any error text before that text can reach a
//!   log line or the UI;
//! - it is only ever sent to `https://api.nexusmods.com` (enforced in
//!   `providers::nexus`), and never crosses the browser bridge.

use std::path::{Path, PathBuf};

use serde::Serialize;

const KEYRING_SERVICE: &str = "io.github.katsyk.ddmm";
const KEYRING_USER: &str = "nexus-api-key";
/// Linux-only fallback file (in DDMM's data folder) used when no Secret
/// Service provider is available.
pub const FALLBACK_FILE_NAME: &str = "nexus-api-key";

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

/// Replace every occurrence of `key` in `text` with `[redacted]`. Applied to
/// every error message that could possibly have been built from a request
/// that carried the key.
pub fn redact(text: &str, key: &NexusApiKey) -> String {
    if key.0.is_empty() {
        return text.to_string();
    }
    text.replace(&key.0, "[redacted]")
}

fn fallback_path(base_path: &Path) -> PathBuf {
    base_path.join(FALLBACK_FILE_NAME)
}

fn keyring_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
}

/// Store `key`, preferring the OS keychain. Returns where it ended up.
pub async fn store(base_path: &Path, key: &NexusApiKey) -> anyhow::Result<KeyStorage> {
    let secret = key.0.clone();
    let keychain = tokio::task::spawn_blocking(move || -> keyring::Result<()> {
        let entry = keyring_entry()?;
        entry.set_password(&secret)?;
        // Read it back: some headless setups accept the write and then
        // can't return it, which would look like "saved" but never work.
        match entry.get_password() {
            Ok(read) if read == secret => Ok(()),
            Ok(_) => Err(keyring::Error::PlatformFailure("stored value didn't read back".into())),
            Err(e) => Err(e),
        }
    })
    .await?;

    match keychain {
        Ok(()) => {
            // A key saved to the keychain supersedes any older fallback file.
            remove_fallback_file(base_path).await;
            Ok(KeyStorage::Keychain)
        }
        Err(e) => {
            // `keyring` errors never contain the secret itself, but scrub
            // anyway -- this text goes to the log.
            let reason = redact(&e.to_string(), key);
            if cfg!(target_os = "linux") {
                log::warn!(
                    "OS keychain (Secret Service) unavailable ({reason}); storing the Nexus API key in an owner-only file in the data folder instead."
                );
                write_fallback_file(base_path, key).await?;
                Ok(KeyStorage::File)
            } else {
                anyhow::bail!("couldn't save the key to the system keychain: {reason}")
            }
        }
    }
}

/// Load the stored key, if any: the fallback file first (it only exists
/// when the keychain wasn't usable at save time), then the keychain.
pub async fn load(base_path: &Path) -> Option<(NexusApiKey, KeyStorage)> {
    if let Some(key) = read_fallback_file(base_path).await {
        return Some((key, KeyStorage::File));
    }

    let loaded = tokio::task::spawn_blocking(|| keyring_entry().and_then(|e| e.get_password()))
        .await
        .ok()?;
    match loaded {
        Ok(secret) => NexusApiKey::parse(&secret).ok().map(|k| (k, KeyStorage::Keychain)),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            log::debug!("No Nexus API key available from the OS keychain: {e}");
            None
        }
    }
}

/// Remove the key from everywhere it could be stored. Never fails: a key
/// that wasn't there is already removed.
pub async fn remove(base_path: &Path) {
    remove_fallback_file(base_path).await;
    let result = tokio::task::spawn_blocking(|| keyring_entry().and_then(|e| e.delete_credential())).await;
    match result {
        Ok(Ok(())) | Ok(Err(keyring::Error::NoEntry)) => {}
        Ok(Err(e)) => log::debug!("Removing the Nexus API key from the OS keychain: {e}"),
        Err(e) => log::debug!("Removing the Nexus API key from the OS keychain: {e}"),
    }
}

async fn write_fallback_file(base_path: &Path, key: &NexusApiKey) -> anyhow::Result<()> {
    let path = fallback_path(base_path);
    // Create with 0600 from the start (not write-then-chmod), so the key is
    // never briefly readable by other users.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let secret = key.0.clone();
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
        tokio::fs::write(&path, key.0.as_bytes()).await?;
    }
    Ok(())
}

async fn read_fallback_file(base_path: &Path) -> Option<NexusApiKey> {
    let data = tokio::fs::read_to_string(fallback_path(base_path)).await.ok()?;
    NexusApiKey::parse(&data).ok()
}

async fn remove_fallback_file(base_path: &Path) {
    let _ = tokio::fs::remove_file(fallback_path(base_path)).await;
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
}
