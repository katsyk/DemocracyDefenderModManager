//! What the startup recovery tells the user (`crate::startup_notices`):
//! one notice per outcome that changed something or needs a look, none on
//! a clean start, and each once per session.

use super::*;

/// The next start's notices.
async fn notices_of(base: &Path) -> Vec<StartupNotice> {
    let mut state_mods = None;
    ensure_mods_loaded_at(&mut state_mods, base).await.unwrap().2.into_vec()
}

fn kinds(notices: &[StartupNotice]) -> Vec<NoticeKind> {
    notices.iter().map(|n| n.kind).collect()
}

fn wrapped_guid() -> Uuid {
    Uuid::parse_str(WRAPPED_GUID).unwrap()
}

#[tokio::test]
async fn a_clean_start_has_no_notices() {
    let base = tempfile::tempdir().unwrap();
    assert!(notices_of(base.path()).await.is_empty(), "first run");

    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let a = install_v1(&state, src.path(), "A.zip", GUID_A, "Alpha").await;
    let old = installed_with_options(&state, src.path(), &["Red", "Blue"]).await;
    write_replace_profiles(base.path(), GUID_A, REPLACE_GUID);
    let update = src.path().join("Cool Mod 2.zip");
    options_mod_zip(&update, REPLACE_GUID, "Cool Mod", &["Red", "Blue", "Green"], b"v2");
    {
        // An update that finishes in the session: nothing for the next start.
        let mut guard = state.mods.lock().await;
        install_update_from_archive(&state, guard.as_mut().unwrap(), &update, old.guid()).await.unwrap();
    }
    assert!(state.take_startup_notices().is_empty());
    drop(state);
    assert!(notices_of(base.path()).await.is_empty());
    assert!(notices_of(base.path()).await.is_empty());
    assert!(a.directory.is_dir());
}

/// The swap happened, profiles.json not yet: finished on the next start,
/// once. Entries without option choices lose none: no "options reset".
#[tokio::test]
async fn an_update_finished_after_a_crash_is_announced_once() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    id_changing_update_stopped_at(base.path(), src.path(), CrashPoint::BeforeProfiles).await;

    let notices = notices_of(base.path()).await;
    assert_eq!(kinds(&notices), [NoticeKind::UpdateFinished], "{notices:?}");
    assert_eq!(notices[0].mod_name.as_deref(), Some("Cool Mod"));
    assert_eq!(notices[0].guid, Some(wrapped_guid()));
    assert_eq!(notices[0].folder.as_deref(), Some(base.path().join(MODS_DIRECTORY).join("Cool Mod").as_path()));
    assert!(notices_of(base.path()).await.is_empty(), "done: nothing on the start after");
}

/// A replace whose new file has other options: the choices the recovery
/// resets are announced, naming the profiles.
#[tokio::test]
async fn options_reset_by_the_recovery_name_the_profiles() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    install_v1(&state, src.path(), "A.zip", GUID_A, "Alpha").await;
    installed_with_options(&state, src.path(), &["Red", "Blue"]).await;
    write_replace_profiles(base.path(), GUID_A, REPLACE_GUID);
    let file = src.path().join("Cool Mod-1234-2-0-1718000000.zip");
    options_mod_zip(&file, REPLACE_GUID, "Cool Mod", &["Red", "Blue", "Green"], b"v2");
    let result = crashing_at(CrashPoint::BeforeProfiles, add_archive(&state, &file, &Asker::Answer(true))).await;
    assert!(result.is_err());
    drop(state);

    let notices = notices_of(base.path()).await;
    assert_eq!(kinds(&notices), [NoticeKind::UpdateFinished, NoticeKind::OptionsReset], "{notices:?}");
    assert_eq!(notices[1].mod_name.as_deref(), Some("Cool Mod"));
    assert_eq!(notices[1].profiles, ["Default", "Other"]);
}

/// Stopped before the new version was in place: the mod is at the version
/// it had, and the user is told so (once).
#[tokio::test]
async fn an_update_undone_after_a_crash_is_announced() {
    for at in [CrashPoint::BeforeSetAside, CrashPoint::BeforeMoveIn] {
        let base = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        id_changing_update_stopped_at(base.path(), src.path(), at).await;

        let notices = notices_of(base.path()).await;
        // Its new files stay hidden, kept (and said so) like any staging
        // copy that isn't the installed mod.
        assert!(kinds(&notices).contains(&NoticeKind::StagingKept), "{at:?}: {notices:?}");
        let undone: Vec<_> = notices.iter().filter(|n| n.kind == NoticeKind::UpdateUndone).collect();
        assert_eq!(undone.len(), 1, "{at:?}: {notices:?}");
        assert_eq!(undone[0].mod_name.as_deref(), Some("Cool Mod"), "{at:?}");
        assert_eq!(undone[0].folder.as_deref(), Some(base.path().join(MODS_DIRECTORY).join("Cool Mod").as_path()));
        assert!(!kinds(&notices_of(base.path()).await).contains(&NoticeKind::UpdateUndone), "{at:?}: not again");
    }
}

/// profiles.json can't be written: said on every start until it can.
#[tokio::test]
async fn profiles_that_cant_be_written_are_announced_until_written() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let old = id_changing_update_stopped_at(base.path(), src.path(), CrashPoint::BeforeProfiles).await;
    std::fs::write(base.path().join("profiles.json"), b"{ not json").unwrap();

    for _ in 0..2 {
        let notices = notices_of(base.path()).await;
        assert_eq!(kinds(&notices), [NoticeKind::ProfilesRetry], "{notices:?}");
        assert_eq!(notices[0].mod_name.as_deref(), Some("Cool Mod"));
    }
    write_profiles(base.path(), &[("Default", &[&old.to_string()])]);
    assert_eq!(kinds(&notices_of(base.path()).await), [NoticeKind::UpdateFinished]);
}

/// The new version's manifest.json unreadable: one notice for the folder
/// (not a second one for the mod it keeps from being listed).
#[tokio::test]
async fn an_update_waiting_on_an_unreadable_manifest_is_one_notice() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    id_changing_update_stopped_at(base.path(), src.path(), CrashPoint::BeforeProfiles).await;
    let folder = base.path().join(MODS_DIRECTORY).join("Cool Mod");
    std::fs::write(folder.join(MANIFEST_FILE), b"{ not json").unwrap();

    let notices = notices_of(base.path()).await;
    assert_eq!(kinds(&notices), [NoticeKind::UpdatePending], "{notices:?}");
    assert_eq!(notices[0].folder.as_deref(), Some(folder.as_path()));
    assert_eq!(notices[0].guid, Some(wrapped_guid()));
}

#[tokio::test]
async fn a_mod_with_an_unreadable_manifest_is_announced_every_start() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(MODS_DIRECTORY);
    write_v1_mod(&root.join("Alpha"), GUID_A, "Alpha");
    let broken = root.join("Broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join(MANIFEST_FILE), b"{ \"Version\": 1,").unwrap();

    for _ in 0..2 {
        let notices = notices_of(base.path()).await;
        assert_eq!(kinds(&notices), [NoticeKind::ModUnreadable], "{notices:?}");
        assert_eq!(notices[0].mod_name.as_deref(), Some("Broken"));
        assert_eq!(notices[0].folder.as_deref(), Some(broken.as_path()));
    }
}

/// A `.adopt-` folder the recovery can't decide on (the mod's folder and
/// the files in the hidden folder both there): named, every start.
#[tokio::test]
async fn a_stuck_in_place_setup_is_announced_every_start() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(MODS_DIRECTORY);
    let stage = root.join(format!("{}{}", mod_folder::IN_PLACE_UNWRAP_PREFIX, Uuid::new_v4()));
    write_v1_mod(&stage.join("Outer").join("Cool Mod"), GUID_A, "Alpha");
    let mut record = stage.file_name().unwrap().to_os_string();
    record.push(mod_folder::IN_PLACE_UNWRAP_RECORD_SUFFIX);
    std::fs::write(stage.with_file_name(record), br#"{"folder":"Cool","root":["Outer","Cool Mod"]}"#).unwrap();
    std::fs::create_dir_all(root.join("Cool")).unwrap();

    for _ in 0..2 {
        let notices = notices_of(base.path()).await;
        assert_eq!(kinds(&notices), [NoticeKind::AdoptLeft], "{notices:?}");
        assert_eq!(notices[0].mod_name.as_deref(), Some("Cool"));
        assert_eq!(notices[0].folder.as_deref(), Some(stage.as_path()));
        assert!(stage.join("Outer").join("Cool Mod").join(MANIFEST_FILE).is_file(), "nothing moved");
    }
}

/// An update backup with nowhere safe to go (its ID is installed in
/// another copy; or it has no manifest): kept, and named every start.
#[tokio::test]
async fn a_backup_that_cant_be_put_back_is_announced_every_start() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(MODS_DIRECTORY);
    write_v1_mod(&root.join("Beta"), GUID_B, "Beta 2");
    let backup = root.join(format!(".update-backup-{}", Uuid::new_v4()));
    write_v1_mod(&backup, GUID_B, "Beta");
    write_record(&backup, "Beta");
    let nameless = root.join(format!(".update-backup-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&nameless).unwrap();
    std::fs::write(nameless.join("file.patch_0"), b"x").unwrap();

    for _ in 0..2 {
        let mut notices = notices_of(base.path()).await;
        notices.sort_by_key(|n| n.mod_name.clone());
        assert_eq!(kinds(&notices), [NoticeKind::BackupKept, NoticeKind::BackupKept], "{notices:?}");
        let named: Vec<_> = notices.iter().map(|n| (n.mod_name.clone().unwrap(), n.folder.clone().unwrap())).collect();
        assert!(named.contains(&("Beta".to_string(), backup.clone())), "{named:?}");
        assert!(named.contains(&(nameless.file_name().unwrap().to_string_lossy().into_owned(), nameless.clone())), "{named:?}");
    }
}

/// A backup put back under another name (no record, its ID not installed):
/// told where it is now.
#[tokio::test]
async fn a_backup_put_back_says_where() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(MODS_DIRECTORY);
    let backup = root.join(format!(".update-backup-{}", Uuid::new_v4()));
    write_v1_mod(&backup, GUID_B, "Beta");

    let notices = notices_of(base.path()).await;
    assert_eq!(kinds(&notices), [NoticeKind::UpdateUndone], "{notices:?}");
    assert_eq!(notices[0].folder.as_deref(), Some(root.join("Beta").as_path()));
    assert!(notices_of(base.path()).await.is_empty());
}

#[tokio::test]
async fn a_kept_staging_copy_and_an_unrecorded_delete_are_announced() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(MODS_DIRECTORY);
    write_v1_mod(&root.join("Beta"), GUID_B, "Beta");
    let staging = root.join(format!(".update-{}", Uuid::new_v4()));
    write_v1_mod(&staging, GUID_B, "Beta 2");
    let identical = root.join(format!(".update-{}", Uuid::new_v4()));
    copy_tree(&root.join("Beta"), &identical);
    let delete = root.join(format!("{}{}", mod_folder::PENDING_DELETE_PREFIX, Uuid::new_v4()));
    write_v1_mod(&delete, GUID_A, "Alpha");

    let notices = notices_of(base.path()).await;
    assert_eq!(kinds(&notices), [NoticeKind::DeleteKept, NoticeKind::StagingKept], "{notices:?}");
    assert_eq!((notices[0].mod_name.as_deref(), notices[0].folder.as_deref()), (Some("Alpha"), Some(delete.as_path())));
    assert_eq!((notices[1].mod_name.as_deref(), notices[1].folder.as_deref()), (Some("Beta 2"), Some(staging.as_path())));
    assert!(!identical.exists(), "an identical copy goes without a word");
    assert_eq!(kinds(&notices_of(base.path()).await), [NoticeKind::DeleteKept, NoticeKind::StagingKept], "every start");
}

/// The session keeps them until the page takes them, once; reading the
/// mod list again adds none; the next start finds the persistent ones
/// again.
#[tokio::test]
async fn notices_are_taken_once_per_session() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(MODS_DIRECTORY);
    write_v1_mod(&root.join("Beta"), GUID_B, "Beta");
    let staging = root.join(format!(".update-{}", Uuid::new_v4()));
    write_v1_mod(&staging, GUID_B, "Beta 2");

    let state = AppState::new(base.path().to_path_buf());
    crate::commands::profiles::load_profiles_for(&state).await.unwrap();
    let taken = state.take_startup_notices();
    assert_eq!(kinds(&taken), [NoticeKind::StagingKept]);
    assert!(state.take_startup_notices().is_empty());
    {
        let mut guard = state.mods.lock().await;
        ensure_mods_loaded(&mut guard, &state).await.unwrap();
    }
    assert!(state.take_startup_notices().is_empty(), "the list is read once");
    assert_eq!(state.startup_notice_folder(&staging), Some(staging.clone()), "its folder can still be opened");
    assert_eq!(state.startup_notice_folder(&root.join("Beta")), None);
    drop(state);

    let next = AppState::new(base.path().to_path_buf());
    crate::commands::profiles::load_profiles_for(&next).await.unwrap();
    assert_eq!(kinds(&next.take_startup_notices()), [NoticeKind::StagingKept], "the next start");
}
