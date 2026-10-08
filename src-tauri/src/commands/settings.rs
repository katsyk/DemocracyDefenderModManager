use std::path::{Path, PathBuf};

use anyhow_tauri::{IntoTAResult, TAResult};
use tauri::State;

use crate::{AppState, models::settings::Settings};

const SETTINGS_FILE: &'static str = "settings.json";

/// The OS Downloads directory, if one can be determined. Used as the
/// default `downloads_path` for the browser handoff (`commands::handoff`).
pub fn default_downloads_path() -> PathBuf {
    dirs::download_dir().unwrap_or_default()
}

/// Read settings.json (or the defaults if there's none yet).
///
/// Called constantly (the auto-import watcher every 2 s, every browser
/// extension request, the update-check scheduler), so it logs at debug
/// level only -- at info it filled and rotated the log file within minutes,
/// wiping the history users attach to bug reports. Failures are returned
/// to the caller, which reports them.
pub async fn do_load_settings(base_path: &Path) -> anyhow::Result<Settings> {
    let settings_file = base_path.join(SETTINGS_FILE);
    log::debug!("Loading settings from {:?}", &settings_file);

    let mut settings = if tokio::fs::try_exists(&settings_file).await? {
        let data = tokio::fs::read(&settings_file).await?;
        serde_json::from_slice(&data)
            .map_err(|e| anyhow::anyhow!("{:?} isn't valid settings JSON: {e}", settings_file))?
    } else {
        log::debug!("No settings file yet; using defaults.");

        Settings::V1 {
            game_path: PathBuf::new(),
            skip_list: vec![],
            downloads_path: default_downloads_path(),
            after_browser_install: "deploy".to_string(),
            bridge_allowed_sites: vec![],
            auto_import_enabled: false,
            auto_check_updates: false,
            auto_check_interval_hours: 0,
            nexus_username: None,
        }
    };

    // Settings files written before `downloads_path` existed deserialize it
    // as an empty path (`#[serde(default)]`); fill in the OS default rather
    // than leaving it unusable.
    if settings.downloads_path().as_os_str().is_empty() {
        settings.set_downloads_path(default_downloads_path());
    }

    Ok(settings)
}

/// Write `settings` to settings.json. Every save goes through here: the
/// file is replaced atomically, because it is read all the time (the
/// auto-import watcher, browser-extension requests, the update scheduler,
/// deploy) and a reader must never see it half written, nor a crash leave
/// it so.
pub async fn write_settings(base_path: &Path, settings: &Settings) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(settings)?;
    crate::fs_util::replace_file(&base_path.join(SETTINGS_FILE), &data).await
}

/// Serializes every read-modify-write of settings.json. `data_op` is a
/// shared lock, so without this two writers (a Settings save and an
/// "Always allow" from the browser extension, say) could each load the
/// file, change their own field and write it back, and the second write
/// would silently undo the first.
static SETTINGS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Hold this while reading, changing and writing settings.json.
pub async fn lock_settings() -> tokio::sync::MutexGuard<'static, ()> {
    SETTINGS_LOCK.lock().await
}

/// Load settings.json, let `change` modify it, and write it back, all
/// under [`lock_settings`]. Returns the settings as written.
pub async fn update_settings(
    base_path: &Path,
    change: impl FnOnce(&mut Settings) -> anyhow::Result<()>,
) -> anyhow::Result<Settings> {
    let _lock = lock_settings().await;
    let mut settings = do_load_settings(base_path).await?;
    change(&mut settings)?;
    write_settings(base_path, &settings).await?;
    Ok(settings)
}

pub async fn do_check_settings(base_path: &Path) -> anyhow::Result<bool> {
    log::info!("Checking settings...");

    match do_load_settings(base_path).await {
        Ok(settings) => {
            match settings.validate().await {
                Ok(root) => {
                    log::info!("Settings valid (game root {:?}).", root);
                    Ok(true)
                }
                Err(e) => {
                    log::error!("Settings invalid: {:#}", e);
                    Ok(false)
                }
            }
        }
        Err(e) => Err(e)
    }
}

/// DDMM's mod storage folder (`<data>/mods`).
fn mods_storage(base_path: &Path) -> PathBuf {
    base_path.join(crate::commands::mods::MODS_DIRECTORY)
}

/// The downloads folder may contain DDMM's data folder (a portable copy
/// kept in Downloads is normal: only files directly in the downloads folder
/// are ever looked at), but must not be the mod storage folder or anything
/// inside it -- downloads would then land among, or inside, installed mods.
pub fn check_downloads_path(base_path: &Path, downloads: &Path) -> anyhow::Result<()> {
    use crate::fs_util::{path_overlap, PathOverlap};
    if downloads.as_os_str().is_empty() {
        return Ok(());
    }
    let storage = mods_storage(base_path);
    match path_overlap(downloads, &storage)? {
        Some(PathOverlap::Same) | Some(PathOverlap::FirstInsideSecond) => anyhow::bail!(
            "The downloads folder ({}) is DDMM's own mod storage or a folder inside it ({}). \
             Pick the folder your browser saves downloads to instead.",
            downloads.display(),
            storage.display()
        ),
        _ => Ok(()),
    }
}

/// The game's `data` folder (where deploy writes, and purge deletes, patch
/// files) must not be DDMM's mod storage or inside it: purging would delete
/// installed mods' own files.
pub fn check_game_data_dir(base_path: &Path, game_data_dir: &Path) -> anyhow::Result<()> {
    use crate::fs_util::{path_overlap, PathOverlap};
    let storage = mods_storage(base_path);
    match path_overlap(game_data_dir, &storage)? {
        Some(PathOverlap::Same) | Some(PathOverlap::FirstInsideSecond) => anyhow::bail!(
            "The game folder's data folder ({}) is inside DDMM's own mod storage ({}). \
             Set the game path to your real Helldivers 2 install in Settings.",
            game_data_dir.display(),
            storage.display()
        ),
        _ => Ok(()),
    }
}

#[tauri::command]
pub async fn load_settings(state: State<'_, AppState>) -> TAResult<Settings> {
    do_load_settings(&state.base_path).await.into_ta_result()
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Settings) -> TAResult<()> {
    let _data_op = state.data_op().into_ta_result()?;
    log::info!("Saving settings...");
    do_save_settings(&state.base_path, settings).await.into_ta_result()?;
    log::info!("Settings saved.");
    Ok(())
}

/// Save the Settings page's `settings`. Fields the backend owns are taken
/// from disk, not from the page, which may have loaded them long ago: the
/// Nexus account name (set when a key is saved or removed) and the
/// browser sites allowed with "Always allow" (changed only by allowing
/// from the extension's prompt or revoking in Settings, each of which
/// writes settings.json itself).
async fn do_save_settings(base_path: &Path, mut settings: Settings) -> anyhow::Result<()> {
    // Store the real game root if the user picked a folder above/below it
    // (e.g. `.../Helldivers 2/data` or `.../steamapps/common`).
    if let Ok(root) = crate::game_path::resolve(settings.game_path()).await {
        if root != settings.game_path() {
            log::info!("Normalizing game path {:?} to {:?}", settings.game_path(), root);
            settings.set_game_path(root);
        }
    }

    check_downloads_path(base_path, settings.downloads_path())?;
    if !settings.game_path().as_os_str().is_empty() {
        check_game_data_dir(base_path, &settings.game_path().join("data"))?;
    }

    let _lock = lock_settings().await;
    match do_load_settings(base_path).await {
        Ok(on_disk) => {
            settings.set_nexus_username(on_disk.nexus_username().map(str::to_string));
            settings.set_bridge_allowed_sites(on_disk.bridge_allowed_sites().to_vec());
        }
        // An unreadable settings.json: nothing to carry over. The account
        // name is shown again once the key is checked; the page's list of
        // allowed sites is the best one left.
        Err(e) => {
            log::warn!("Couldn't read the current settings before saving: {e:#}");
            settings.set_nexus_username(None);
        }
    }
    write_settings(base_path, &settings).await
}

#[tauri::command]
pub  async fn check_settings(state: State<'_, AppState>) -> TAResult<bool> {
    do_check_settings(&state.base_path).await.into_ta_result()
}

/// The directory DDMM is currently keeping `mods/`, `settings.json`,
/// `profiles.json` and logs in -- shown read-only in Settings with an
/// "Open folder" button.
#[tauri::command]
pub fn get_data_dir(state: State<'_, AppState>) -> String {
    state.base_path.to_string_lossy().into_owned()
}

/// Check a game path the user is typing/picking in Settings and say
/// exactly what's wrong with it (or which folder will actually be used).
/// Done here rather than in the frontend so the result doesn't depend on
/// Tauri's fs scope -- see `crate::game_path`.
#[tauri::command]
pub async fn validate_game_path(path: String) -> crate::game_path::GamePathReport {
    let input = PathBuf::from(&path);
    let result = crate::game_path::resolve(&input).await;
    let report = crate::game_path::GamePathReport::from_result(&input, &result);
    match &result {
        Ok(root) if root != &input => log::info!("Game path {:?} resolves to {:?}", input, root),
        Ok(_) => {}
        Err(_) => log::warn!("Game path check: {}", report.message.as_deref().unwrap_or("invalid")),
    }
    report
}

/// Look for a Helldivers 2 install via Steam, without touching settings.
/// Backs the "Auto-detect" button in Settings.
#[tauri::command]
pub async fn detect_game_path() -> Option<String> {
    crate::steam::detect_game_path()
        .await
        .map(|p| p.to_string_lossy().into_owned())
}

/// Detect a Helldivers 2 install and, if found, save it as the game path
/// immediately. Used on first run (and whenever settings are otherwise
/// invalid) so a new player never has to open Settings by hand. Returns the
/// detected path, if any, so the caller can tell the user where it found
/// it.
#[tauri::command]
pub async fn auto_detect_and_save_game_path(state: State<'_, AppState>) -> TAResult<Option<String>> {
    let _data_op = state.data_op().into_ta_result()?;
    let Some(path) = crate::steam::detect_game_path().await else {
        return Ok(None);
    };

    update_settings(&state.base_path, |settings| {
        settings.set_game_path(path.clone());
        Ok(())
    })
    .await
    .into_ta_result()?;

    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// settings.json is read all the time (the auto-import watcher, every
    /// browser-extension request, the update scheduler, deploy). A save
    /// must never let one of those reads see a half-written file -- nor
    /// leave one behind if DDMM dies mid-write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readers_never_see_a_half_written_settings_file() {
        let base = tempfile::tempdir().unwrap();
        let mut settings = do_load_settings(base.path()).await.unwrap();
        // Big enough that a write isn't a single syscall's worth.
        settings.set_game_path(PathBuf::from("x".repeat(256 * 1024)));
        write_settings(base.path(), &settings).await.unwrap();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (base, stop) = (base.path().to_path_buf(), stop.clone());
            tokio::spawn(async move {
                let mut failures = 0;
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    if do_load_settings(&base).await.is_err() {
                        failures += 1;
                    }
                }
                failures
            })
        };
        for _ in 0..200 {
            write_settings(base.path(), &settings).await.unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(reader.await.unwrap(), 0, "a read saw a half-written settings.json");
    }

    /// The Settings page is open with settings it loaded earlier; meanwhile
    /// the browser extension's "Always allow" (and a Nexus key) are saved.
    /// Saving the page must not undo them.
    #[tokio::test]
    async fn saving_the_settings_page_keeps_backend_owned_fields() {
        let base = tempfile::tempdir().unwrap();
        let mut page = do_load_settings(base.path()).await.unwrap();
        write_settings(base.path(), &page).await.unwrap();

        update_settings(base.path(), |s| {
            s.allow_bridge_site("example.com".into());
            s.set_nexus_username(Some("Diver".into()));
            Ok(())
        })
        .await
        .unwrap();

        // The page's stale copy, with one change the user did make.
        page.set_downloads_path(base.path().to_path_buf());
        do_save_settings(base.path(), page).await.unwrap();

        let saved = do_load_settings(base.path()).await.unwrap();
        assert_eq!(saved.bridge_allowed_sites(), ["example.com".to_string()]);
        assert_eq!(saved.nexus_username(), Some("Diver"));
        assert_eq!(saved.downloads_path(), base.path());
    }

    /// Concurrent read-modify-write updates must all land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_settings_updates_are_not_lost() {
        let base = tempfile::tempdir().unwrap();
        write_settings(base.path(), &do_load_settings(base.path()).await.unwrap()).await.unwrap();
        let tasks: Vec<_> = (0..20)
            .map(|i| {
                let base = base.path().to_path_buf();
                tokio::spawn(async move {
                    update_settings(&base, |s| {
                        s.allow_bridge_site(format!("site{i}.example"));
                        Ok(())
                    })
                    .await
                    .unwrap();
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        let saved = do_load_settings(base.path()).await.unwrap();
        assert_eq!(saved.bridge_allowed_sites().len(), 20, "{:?}", saved.bridge_allowed_sites());
    }

    #[test]
    fn downloads_folder_may_contain_the_data_folder_but_not_be_inside_the_mod_storage() {
        let downloads = tempfile::tempdir().unwrap();
        // A portable DDMM kept in Downloads: fine.
        let base = downloads.path().join("DDMM");
        std::fs::create_dir_all(base.join("mods").join("ModA")).unwrap();
        check_downloads_path(&base, downloads.path()).unwrap();
        check_downloads_path(&base, &base).unwrap();
        check_downloads_path(&base, Path::new("")).unwrap();

        assert!(check_downloads_path(&base, &base.join("mods")).is_err());
        assert!(check_downloads_path(&base, &base.join("mods").join("ModA")).is_err());
        assert!(check_downloads_path(&base, &base.join("mods").join("ModA").join("..")).is_err());
    }

    #[test]
    fn game_data_folder_must_not_be_inside_the_mod_storage() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("mods")).unwrap();
        let game = tempfile::tempdir().unwrap();
        check_game_data_dir(base.path(), &game.path().join("data")).unwrap();
        // A portable DDMM inside the game's data folder is harmless.
        check_game_data_dir(base.path(), base.path()).unwrap();

        assert!(check_game_data_dir(base.path(), &base.path().join("mods")).is_err());
        assert!(check_game_data_dir(base.path(), &base.path().join("mods").join("x").join("data")).is_err());
    }
}