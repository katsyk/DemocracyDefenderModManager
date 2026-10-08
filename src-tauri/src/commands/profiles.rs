use anyhow_tauri::{IntoTAResult, TAResult};
use tauri::State;

use crate::{AppState, models::{manifest::Manifest, profile::{Config, Profile, ProfilesConfig}}};

const PROFILES_FILE: &'static str = "profiles.json";

pub async fn do_load_profiles(base_path: &std::path::Path) -> anyhow::Result<ProfilesConfig> {
    let profiles_file = base_path.join(PROFILES_FILE);

    log::info!("Loading profiles...");

    log::info!("Looking for {:?}", &profiles_file);
    let mut profiles = if tokio::fs::try_exists(&profiles_file).await? {
        log::info!("Opening...");
        let data = tokio::fs::read(profiles_file).await?;

        log::info!("Deserializing...");
        serde_json::from_slice(&data)?
    } else {
        log::info!("Not found. Using default.");

        ProfilesConfig {
            profiles: vec![
                Profile::V1 {
                    name: "Default".to_string(),
                    configs: Vec::new()
                }
            ],
            active: 0
        }
    };

    let repeated = remove_repeated_entries(&mut profiles);
    if repeated > 0 {
        log::warn!("profiles.json listed some mods more than once in a profile; ignoring {repeated} extra entr(ies) (the first of each is kept).");
    }

    log::info!("Profiles loaded.");
    Ok(profiles)
}

/// Drop every entry of a mod after its first one in each profile; returns
/// how many were dropped. Up to 2.0.0-rc.9 the Mods page could save a mod
/// twice in a profile (issue #71); everything that reads profiles.json gets
/// it without the repeats (the page also repairs it, and its next save
/// writes the file without them).
fn remove_repeated_entries(config: &mut ProfilesConfig) -> usize {
    let mut removed = 0;
    for profile in &mut config.profiles {
        match profile {
            Profile::V1 { configs, .. } => {
                let mut seen = std::collections::HashSet::new();
                let before = configs.len();
                configs.retain(|c| seen.insert(*c.uuid()));
                removed += before - configs.len();
            }
        }
    }
    removed
}

#[tauri::command]
pub async fn load_profiles(state: State<'_, AppState>) -> TAResult<ProfilesConfig> {
    do_load_profiles(&state.base_path).await.into_ta_result()
}

#[tauri::command]
pub async fn save_profiles(state: State<'_, AppState>, config: ProfilesConfig) -> TAResult<()> {
    let _data_op = state.data_op().into_ta_result()?;
    log::info!("Saving profiles...");
    
    if log::log_enabled!(log::Level::Debug) {
        for profile in &config.profiles {
            log::debug!("Profile \"{}\" Order:", profile.name());
            for config in profile.configs() {
                log::debug!("- {{{}}}", config.uuid());
            }
        }
    }

    let mut config = config;
    apply_guid_renames(&mut config, &state.guid_renames());

    let _saving = PROFILES_WRITE.lock().await;
    write_profiles(&state.base_path, &config).await.into_ta_result()?;

    log::info!("Profiles saved.");

    Ok(())
}

/// The mods whose ID changed in an update this session, as (old, new) in
/// the order they happened. The Mods page moves its entries over with it.
#[tauri::command]
pub async fn get_guid_renames(state: State<'_, AppState>) -> TAResult<Vec<(uuid::Uuid, uuid::Uuid)>> {
    Ok(state.guid_renames())
}

/// Move entries still listing a mod by an ID it had before an update this
/// session (a page that hadn't caught up yet) over to its new ID.
pub(crate) fn apply_guid_renames(config: &mut ProfilesConfig, renames: &[(uuid::Uuid, uuid::Uuid)]) {
    if renames.is_empty() {
        return;
    }
    let mut renamed = false;
    for profile in &mut config.profiles {
        match profile {
            Profile::V1 { configs, .. } => {
                for c in configs.iter_mut() {
                    for (old, new) in renames {
                        if c.uuid() == old {
                            c.set_uuid(*new);
                            renamed = true;
                        }
                    }
                }
            }
        }
    }
    if renamed {
        remove_repeated_entries(config);
    }
}

/// Take every entry of the mod `guid` out of every profile in the saved
/// profiles.json, and return how many there were. Called when a mod is
/// deleted, so that a deleted mod never comes back as "Mod not found" --
/// even if DDMM is closed before the frontend saves its profiles again.
/// Nothing is written when no profile has it (or there's no file yet).
pub async fn remove_from_saved_profiles(base_path: &std::path::Path, guid: uuid::Uuid) -> anyhow::Result<usize> {
    let _saving = PROFILES_WRITE.lock().await;
    if !tokio::fs::try_exists(base_path.join(PROFILES_FILE)).await? {
        return Ok(0);
    }
    let mut config = do_load_profiles(base_path).await?;
    let mut removed = 0;
    for profile in &mut config.profiles {
        match profile {
            Profile::V1 { configs, .. } => {
                let before = configs.len();
                configs.retain(|c| *c.uuid() != guid);
                removed += before - configs.len();
            }
        }
    }
    if removed > 0 {
        write_profiles(base_path, &config).await?;
    }
    Ok(removed)
}

/// Move every entry of the mod `old` in the saved profiles.json over to
/// `new` (the ID the mod has after an update), and return how many there
/// were. On/off and position are kept; option choices that no longer fit
/// `manifest` are reset to the defaults (see [`fit_config`]). A profile
/// that already lists `new` keeps whichever entry comes first. Nothing is
/// written when no profile has `old` (or there's no file yet).
pub async fn migrate_saved_profiles(
    base_path: &std::path::Path,
    old: uuid::Uuid,
    new: uuid::Uuid,
    manifest: &Manifest,
) -> anyhow::Result<usize> {
    let _saving = PROFILES_WRITE.lock().await;
    if !tokio::fs::try_exists(base_path.join(PROFILES_FILE)).await? {
        return Ok(0);
    }
    let mut config = do_load_profiles(base_path).await?;
    let mut moved = 0;
    for profile in &mut config.profiles {
        match profile {
            Profile::V1 { configs, .. } => {
                for c in configs.iter_mut().filter(|c| *c.uuid() == old) {
                    *c = fit_config(c, new, manifest);
                    moved += 1;
                }
            }
        }
    }
    if moved > 0 {
        remove_repeated_entries(&mut config);
        write_profiles(base_path, &config).await?;
    }
    Ok(moved)
}

/// `config` for the mod `guid` whose manifest is `manifest`: its option
/// choices when they still fit (same manifest version, same number of
/// options, sub-option choices in range), else every option on and the
/// first choice everywhere -- like a new entry, but keeping on/off.
pub(crate) fn fit_config(config: &Config, guid: uuid::Uuid, manifest: &Manifest) -> Config {
    let enabled = config.enabled();
    let in_range = |i: usize, count: usize| i < count.max(1);
    // Sub-option counts per option.
    let subs: Option<Vec<usize>> = match manifest {
        Manifest::Legacy(_) => None,
        Manifest::V1(m) => Some(m.options.iter().flatten().map(|o| o.sub_options.as_ref().map_or(0, Vec::len)).collect()),
        Manifest::V2(m) => Some(m.options.iter().flatten().map(|o| o.sub_options.as_ref().map_or(0, Vec::len)).collect()),
    };
    match (manifest, config) {
        (Manifest::Legacy(m), Config::Legacy { selected, .. }) if in_range(*selected, m.options.as_ref().map_or(0, Vec::len)) => {
            Config::Legacy { guid, enabled, selected: *selected }
        }
        (Manifest::Legacy(_), _) => Config::Legacy { guid, enabled, selected: 0 },
        (_, Config::V1 { toggled, selected, .. }) | (_, Config::V2 { toggled, selected, .. }) => {
            let subs = subs.unwrap_or_default();
            let same_version = matches!((manifest, config), (Manifest::V1(_), Config::V1 { .. }) | (Manifest::V2(_), Config::V2 { .. }));
            let fits = same_version
                && toggled.len() == subs.len()
                && selected.len() == subs.len()
                && selected.iter().zip(&subs).all(|(s, n)| in_range(*s, *n));
            if fits {
                with_version(manifest, guid, enabled, toggled.clone(), selected.clone())
            } else {
                with_version(manifest, guid, enabled, vec![true; subs.len()], vec![0; subs.len()])
            }
        }
        (_, Config::Legacy { .. }) => {
            let n = subs.map_or(0, |s| s.len());
            with_version(manifest, guid, enabled, vec![true; n], vec![0; n])
        }
    }
}

fn with_version(manifest: &Manifest, guid: uuid::Uuid, enabled: bool, toggled: Vec<bool>, selected: Vec<usize>) -> Config {
    match manifest {
        Manifest::V2(_) => Config::V2 { guid, enabled, toggled, selected },
        _ => Config::V1 { guid, enabled, toggled, selected },
    }
}

/// Held while profiles.json is written (or read to be rewritten), so a
/// save from the page and a delete's own update of the file never
/// interleave.
static PROFILES_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Write profiles.json atomically: into a temporary file next to it, then
/// renamed over it (retried while another program has it open), so it's
/// never seen (or left, on a crash) half written.
async fn write_profiles(base_path: &std::path::Path, config: &ProfilesConfig) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(config)?;
    crate::fs_util::replace_file(&base_path.join(PROFILES_FILE), &data).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_util::replace_file_with;

    fn profiles_with(guids: &[&str]) -> ProfilesConfig {
        let configs = guids
            .iter()
            .map(|g| serde_json::json!({ "For": "V1", "Guid": g, "Enabled": true, "Toggled": [], "Selected": [] }))
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({ "Profiles": [{ "Version": "V1", "Name": "Default", "Configs": configs }], "Active": 0 }))
            .unwrap()
    }

    const A: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
    const B: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

    #[tokio::test]
    async fn loading_drops_repeated_entries_keeping_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = profiles_with(&[A, B, A, "AAAAAAAA-1111-4111-8111-AAAAAAAAAAAA", B]);
        let Profile::V1 { configs, .. } = &mut config.profiles[0];
        // The first entry of A is off; that's the one kept.
        configs[0] = serde_json::from_value(serde_json::json!({ "For": "V1", "Guid": A, "Enabled": false, "Toggled": [], "Selected": [] })).unwrap();
        write_profiles(dir.path(), &config).await.unwrap();

        let loaded = do_load_profiles(dir.path()).await.unwrap();
        let configs = loaded.profiles[0].configs();
        let guids: Vec<String> = configs.iter().map(|c| c.uuid().to_string()).collect();
        assert_eq!(guids, [A, B]);
        assert!(!configs[0].enabled());
    }

    /// Saves and a delete's removal running at once (all without the
    /// deleted mod, as the page's saves are once the delete returned):
    /// the result is a whole, valid file without it, and no temporary file
    /// is left.
    #[tokio::test]
    async fn concurrent_writes_leave_a_whole_file_and_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        write_profiles(dir.path(), &profiles_with(&[A, B])).await.unwrap();
        let mut tasks = Vec::new();
        for i in 0..20 {
            let base = dir.path().to_path_buf();
            tasks.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    let _saving = PROFILES_WRITE.lock().await;
                    write_profiles(&base, &profiles_with(&[A])).await.unwrap();
                } else {
                    remove_from_saved_profiles(&base, uuid::Uuid::parse_str(B).unwrap()).await.unwrap();
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let loaded = do_load_profiles(dir.path()).await.unwrap();
        let guids: Vec<String> = loaded.profiles[0].configs().iter().map(|c| c.uuid().to_string()).collect();
        assert_eq!(guids, vec![A.to_string()]);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![PROFILES_FILE.to_string()]);
    }

    fn in_use() -> std::io::Error {
        std::io::Error::from_raw_os_error(32) // ERROR_SHARING_VIOLATION on Windows
    }

    const FAST: &[std::time::Duration] = &[std::time::Duration::from_millis(1); 3];

    /// The file is busy for a moment (Windows antivirus/sync client): the
    /// rename is retried and the save goes through.
    #[tokio::test]
    async fn a_briefly_busy_profiles_file_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        let tries = std::sync::atomic::AtomicUsize::new(0);
        replace_file_with(&dir.path().join(PROFILES_FILE), &serde_json::to_vec_pretty(&profiles_with(&[A])).unwrap(), FAST, |e| e.raw_os_error() == Some(32), |from, to| {
            let n = tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { if n < 2 { Err(in_use()) } else { tokio::fs::rename(from, to).await } }
        })
        .await
        .unwrap();
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 3);
        let loaded = do_load_profiles(dir.path()).await.unwrap();
        assert_eq!(loaded.profiles[0].configs().len(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left");
    }

    /// Still busy after every retry: a clear error, the old file untouched
    /// and no temp file left. Other errors aren't retried at all.
    #[tokio::test]
    async fn a_profiles_file_that_stays_busy_fails_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        write_profiles(dir.path(), &profiles_with(&[A, B])).await.unwrap();
        let tries = std::sync::atomic::AtomicUsize::new(0);
        let err = replace_file_with(&dir.path().join(PROFILES_FILE), &serde_json::to_vec_pretty(&profiles_with(&[A])).unwrap(), FAST, |e| e.raw_os_error() == Some(32), |_, _| {
            tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Err(in_use()) }
        })
        .await
        .unwrap_err();
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 4, "first try + 3 retries");
        assert!(format!("{err:#}").contains("in use by another program"), "{err:#}");
        assert_eq!(do_load_profiles(dir.path()).await.unwrap().profiles[0].configs().len(), 2);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left");

        tries.store(0, std::sync::atomic::Ordering::SeqCst);
        let err = replace_file_with(&dir.path().join(PROFILES_FILE), &serde_json::to_vec_pretty(&profiles_with(&[A])).unwrap(), FAST, |e| e.raw_os_error() == Some(32), |_, _| {
            tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Err(std::io::Error::from(std::io::ErrorKind::NotFound)) }
        })
        .await
        .unwrap_err();
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!format!("{err:#}").contains("in use by another program"), "{err:#}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left");
    }

    /// Real Windows behaviour: profiles.json held open without delete
    /// sharing (as a scanner does) makes the rename fail; once it's closed
    /// within the retry window, the save goes through.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_save_waits_for_a_scanner_to_close_profiles_json() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let dir = tempfile::tempdir().unwrap();
        write_profiles(dir.path(), &profiles_with(&[A, B])).await.unwrap();
        let path = dir.path().join(PROFILES_FILE);

        let held = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).unwrap();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held);
        });
        write_profiles(dir.path(), &profiles_with(&[A])).await.unwrap();
        closer.join().unwrap();
        assert_eq!(do_load_profiles(dir.path()).await.unwrap().profiles[0].configs().len(), 1);

        // Held open the whole time: fails cleanly, old file kept.
        let held = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).unwrap();
        let err = write_profiles(dir.path(), &profiles_with(&[A, B])).await.unwrap_err();
        drop(held);
        assert!(format!("{err:#}").contains("in use by another program"), "{err:#}");
        assert_eq!(do_load_profiles(dir.path()).await.unwrap().profiles[0].configs().len(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left");
    }

    #[tokio::test]
    async fn removing_from_profiles_without_a_file_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(remove_from_saved_profiles(dir.path(), uuid::Uuid::parse_str(A).unwrap()).await.unwrap(), 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
