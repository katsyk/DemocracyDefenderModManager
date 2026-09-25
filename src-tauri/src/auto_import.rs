//! Opt-in, off by default: while enabled (`Settings::auto_import_enabled`),
//! watches the Downloads folder the whole time DDMM is running (not just
//! during a [`crate::commands::handoff`]) for a new, finished archive that
//! looks like a Helldivers 2 mod, and asks the frontend to offer installing
//! it. Never installs anything itself -- see
//! `docs/using/one-click-install.md#auto-import-from-downloads`.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use tauri::{AppHandle, Emitter, Manager};

use crate::{
    archive::Archive,
    commands::{
        handoff::{ARCHIVE_SUFFIXES, IGNORED_SUFFIXES},
        settings::do_load_settings,
    },
    utils::is_patch_filename,
    AppState,
};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Start the background watcher. Runs for the app's whole lifetime; every
/// tick re-reads settings, so toggling auto-import on/off in Settings
/// takes effect within one poll interval with nothing else to wire up.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(run(app));
}

async fn run(app: AppHandle) {
    // Files already reported (skipped on every later tick) and files seen
    // but not yet confirmed stable (path -> last observed size).
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut pending: HashMap<PathBuf, u64> = HashMap::new();
    // `false` right after startup and right after resuming from a pause
    // (an active handoff) -- the next tick re-baselines instead of
    // evaluating, so anything already there (including whatever a handoff
    // just finished downloading) is never flagged as new.
    let mut baseline_ready = false;
    let mut settings_error: Option<String> = None;

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        let state = app.state::<AppState>();

        // The one active handoff (if any) polls this same folder for its
        // own candidate; staying out of its way entirely is simpler and
        // safer than trying to distinguish "its" file from any other.
        if state.handoff_cancel.lock().await.is_some() {
            baseline_ready = false;
            continue;
        }

        let settings = match do_load_settings(&state.base_path).await {
            Ok(s) => {
                settings_error = None;
                s
            }
            Err(e) => {
                // Log a broken settings file once, not every tick.
                let message = format!("{e:#}");
                if settings_error.as_deref() != Some(message.as_str()) {
                    log::warn!("Auto-import: couldn't read settings: {message}");
                    settings_error = Some(message);
                }
                continue;
            }
        };
        if !settings.auto_import_enabled() {
            baseline_ready = false;
            pending.clear();
            continue;
        }

        let Ok(candidates) = list_candidates(settings.downloads_path()).await else {
            continue;
        };

        if !baseline_ready {
            seen = candidates.into_iter().collect();
            pending.clear();
            baseline_ready = true;
            continue;
        }

        // Drop pending/seen entries for files that disappeared (moved,
        // deleted, or already installed by something else).
        let current: HashSet<&PathBuf> = candidates.iter().collect();
        pending.retain(|p, _| current.contains(p));

        for path in candidates {
            if seen.contains(&path) {
                continue;
            }

            let Ok(size) = tokio::fs::metadata(&path).await.map(|m| m.len()) else {
                continue;
            };

            match pending.get(&path) {
                // Same size as last tick: the download has finished.
                Some(&last_size) if last_size == size => {
                    pending.remove(&path);
                    seen.insert(path.clone());

                    if looks_like_hd2_mod_archive(path.clone()).await {
                        let _ = app.emit(
                            "auto-import://candidate",
                            serde_json::json!({ "file": path.to_string_lossy() }),
                        );
                    }
                }
                // First sighting, or still growing: wait for the next tick.
                _ => {
                    pending.insert(path, size);
                }
            }
        }
    }
}

/// Every plain (non-symlink), non-partial-download, archive-extension file
/// currently in `dir`. Mirrors `commands::handoff::find_new_archive`'s
/// filtering, minus the "new since a start time" part -- this module
/// tracks novelty across polls itself instead.
async fn list_candidates(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return Ok(out), // missing/inaccessible downloads folder: nothing to offer, not an error
    };

    while let Some(entry) = entries.next_entry().await? {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();

        if IGNORED_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
            continue;
        }
        if !ARCHIVE_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
            continue;
        }

        let path = entry.path();
        let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }

        out.push(path);
    }

    Ok(out)
}

/// Peek inside the archive -- via [`Archive::iter`], without extracting
/// anything -- for an entry whose name matches a Helldivers 2 patch file.
/// Blocking archive I/O runs on a blocking thread so it never stalls the
/// async runtime; any error (corrupt/unreadable archive, still mid-write
/// despite the size-stability check) is just "not a candidate", not
/// reported anywhere -- a real problem still surfaces normally if the user
/// chooses to install it anyway.
async fn looks_like_hd2_mod_archive(path: PathBuf) -> bool {
    tokio::task::spawn_blocking(move || {
        let Ok(mut archive) = Archive::open(&path) else {
            return false;
        };
        let Ok(iter) = archive.iter() else {
            return false;
        };
        for entry in iter {
            let Ok(entry) = entry else { continue };
            if entry.is_directory() || entry.is_symlink() {
                continue;
            }
            if let Some(name) = entry.path().file_name().and_then(|n| n.to_str()) {
                if is_patch_filename(name) {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[tokio::test]
    async fn list_candidates_filters_like_handoff_does() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("mod.zip"), b"data").await.unwrap();
        tokio::fs::write(dir.path().join("still-downloading.zip.crdownload"), b"").await.unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), b"data").await.unwrap();

        let found = list_candidates(dir.path()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file_name().unwrap(), "mod.zip");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_candidates_ignores_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.zip");
        tokio::fs::write(&real, b"data").await.unwrap();
        tokio::fs::symlink(&real, dir.path().join("link.zip")).await.unwrap();

        let found = list_candidates(dir.path()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file_name().unwrap(), "real.zip");
    }

    #[tokio::test]
    async fn list_candidates_missing_dir_is_empty_not_error() {
        let found = list_candidates(Path::new("/nonexistent/does/not/exist")).await.unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn looks_like_hd2_mod_archive_true_for_patch_file() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("mod.zip");
        make_zip(&zip_path, &[("0123456789abcdef.patch_0", b"data")]);

        assert!(looks_like_hd2_mod_archive(zip_path).await);
    }

    #[tokio::test]
    async fn looks_like_hd2_mod_archive_false_for_unrelated_zip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("readme.zip");
        make_zip(&zip_path, &[("readme.txt", b"data")]);

        assert!(!looks_like_hd2_mod_archive(zip_path).await);
    }

    #[tokio::test]
    async fn looks_like_hd2_mod_archive_false_for_non_archive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mod.zip");
        tokio::fs::write(&path, b"not a zip").await.unwrap();

        assert!(!looks_like_hd2_mod_archive(path).await);
    }
}
