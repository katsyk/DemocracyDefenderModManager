use anyhow_tauri::{IntoTAResult, TAResult};
use tauri::State;

use crate::{AppState, models::profile::{Profile, ProfilesConfig}};

const PROFILES_FILE: &'static str = "profiles.json";

pub async fn do_load_profiles(base_path: &std::path::Path) -> anyhow::Result<ProfilesConfig> {
    let profiles_file = base_path.join(PROFILES_FILE);

    log::info!("Loading profiles...");

    log::info!("Looking for {:?}", &profiles_file);
    let profiles = if tokio::fs::try_exists(&profiles_file).await? {
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

    log::info!("Profiles loaded.");
    Ok(profiles)
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

    let _saving = PROFILES_WRITE.lock().await;
    write_profiles(&state.base_path, &config).await.into_ta_result()?;

    log::info!("Profiles saved.");

    Ok(())
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

/// Held while profiles.json is written (or read to be rewritten), so a
/// save from the page and a delete's own update of the file never
/// interleave.
static PROFILES_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long to wait before each retry of a failed rename over
/// profiles.json (about 1 s in all).
const RENAME_RETRY_DELAYS: &[std::time::Duration] = &[
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(300),
    std::time::Duration::from_millis(600),
];

/// Whether a failed rename over profiles.json is worth retrying: on
/// Windows, an antivirus scanner, search indexer or sync client that has
/// the file open without delete sharing makes the rename fail for a moment
/// (access denied, sharing or lock violation). Nothing else is retried.
fn is_transient_rename_error(e: &std::io::Error) -> bool {
    cfg!(windows) && e.raw_os_error().is_some_and(is_windows_file_in_use_code)
}

/// ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
fn is_windows_file_in_use_code(code: i32) -> bool {
    matches!(code, 5 | 32 | 33)
}

/// Write profiles.json atomically: into a temporary file next to it, then
/// renamed over it, so it's never seen (or left, on a crash) half written.
async fn write_profiles(base_path: &std::path::Path, config: &ProfilesConfig) -> anyhow::Result<()> {
    write_profiles_with(base_path, config, RENAME_RETRY_DELAYS, is_transient_rename_error, |from, to| {
        tokio::fs::rename(from, to)
    })
    .await
}

/// [`write_profiles`] with the retry delays, the "retry this?" test and the
/// rename step injectable, so the retries can be tested on every platform.
async fn write_profiles_with<R, Fut>(
    base_path: &std::path::Path,
    config: &ProfilesConfig,
    delays: &[std::time::Duration],
    retryable: impl Fn(&std::io::Error) -> bool,
    rename: R,
) -> anyhow::Result<()>
where
    R: Fn(std::path::PathBuf, std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<()>>,
{
    let data = serde_json::to_vec_pretty(config)?;
    let file = base_path.join(PROFILES_FILE);
    let temp = base_path.join(format!("{PROFILES_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let written = async {
        tokio::fs::write(&temp, &data).await?;
        let mut delays = delays.iter();
        loop {
            match rename(temp.clone(), file.clone()).await {
                Ok(()) => return Ok(()),
                Err(e) if retryable(&e) => match delays.next() {
                    Some(delay) => {
                        log::info!("Replacing {:?} failed ({e}); another program may have it open. Trying again.", file);
                        tokio::time::sleep(*delay).await;
                    }
                    None => return Err(e),
                },
                Err(e) => return Err(e),
            }
        }
    }
    .await;
    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&temp).await;
        let in_use = retryable(&e);
        let e = anyhow::Error::from(e).context(format!("couldn't write {:?}", file));
        return Err(if in_use {
            e.context("profiles.json is in use by another program (an antivirus or sync app?); close it or try again")
        } else {
            e
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        write_profiles_with(dir.path(), &profiles_with(&[A]), FAST, |e| e.raw_os_error() == Some(32), |from, to| {
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
        let err = write_profiles_with(dir.path(), &profiles_with(&[A]), FAST, |e| e.raw_os_error() == Some(32), |_, _| {
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
        let err = write_profiles_with(dir.path(), &profiles_with(&[A]), FAST, |e| e.raw_os_error() == Some(32), |_, _| {
            tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Err(std::io::Error::from(std::io::ErrorKind::NotFound)) }
        })
        .await
        .unwrap_err();
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!format!("{err:#}").contains("in use by another program"), "{err:#}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left");
    }

    #[test]
    fn only_windows_file_in_use_errors_are_retried() {
        assert!(is_windows_file_in_use_code(5));
        assert!(is_windows_file_in_use_code(32));
        assert!(is_windows_file_in_use_code(33));
        assert!(!is_windows_file_in_use_code(2));
        assert!(!is_transient_rename_error(&std::io::Error::from(std::io::ErrorKind::NotFound)));
        #[cfg(not(windows))]
        assert!(!is_transient_rename_error(&in_use()), "not retried off Windows");
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
