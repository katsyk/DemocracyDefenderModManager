use super::*;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

fn no_free_space_info(_: &Path) -> Option<u64> {
    None
}

const SETTINGS: &str = r#"{"Version":"V1","GamePath":"","SkipList":[]}"#;
const PROFILES: &str = r#"{"Profiles":[{"Version":"V1","Name":"Default","Configs":[]}],"Active":0}"#;

/// A small but realistic data folder.
fn make_data(dir: &Path) {
    std::fs::create_dir_all(dir.join("mods/ModA/Options/Red")).unwrap();
    std::fs::write(dir.join("mods/ModA/manifest.json"), b"{}").unwrap();
    std::fs::write(dir.join("mods/ModA/Options/Red/0123456789abcdef.patch_0"), vec![7u8; 4096]).unwrap();
    std::fs::create_dir_all(dir.join("mods/ModB")).unwrap();
    std::fs::write(dir.join("mods/ModB/.hd2mm-origin.json"), b"{}").unwrap();
    std::fs::write(dir.join("settings.json"), SETTINGS).unwrap();
    std::fs::write(dir.join("profiles.json"), PROFILES).unwrap();
    std::fs::write(dir.join("update-cache.json"), b"{}").unwrap();
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    std::fs::write(dir.join("logs/ddmm.log"), b"log line\n").unwrap();
    std::fs::create_dir_all(dir.join(".downloads/abc")).unwrap();
    std::fs::create_dir_all(dir.join("native-messaging")).unwrap();
    std::fs::write(dir.join("bridge.json"), b"{}").unwrap();
}

fn input<'a>(current: &'a Path, picked: &'a Path, default_path: &'a Path) -> PlanInput<'a> {
    PlanInput { current, picked, default_path, game_path: None, reset: false }
}

fn plan_err(input: &PlanInput) -> String {
    format!("{:#}", plan(input, &no_free_space_info).unwrap_err())
}

fn exdev() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(18)
    }
    #[cfg(windows)]
    {
        io::Error::from_raw_os_error(17)
    }
}

/// Every file under `dir`, relative, with its contents.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.insert(e.path().strip_prefix(dir).unwrap().to_path_buf(), std::fs::read(e.path()).unwrap());
            }
        }
    }
    out
}

fn no_commit(_: &Path) -> anyhow::Result<()> {
    Ok(())
}

// ---- validation -------------------------------------------------------

#[test]
fn rejects_the_current_folder_itself() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let msg = plan_err(&input(&data, &data, &root.path().join("default")));
    assert!(msg.contains("already DDMM's data folder"), "{msg}");
}

#[test]
fn rejects_a_folder_inside_the_current_one() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let inside = data.join("mods");
    let msg = plan_err(&input(&data, &inside, &root.path().join("default")));
    assert!(msg.contains("inside DDMM's current data folder"), "{msg}");
}

#[test]
fn rejects_a_folder_containing_the_current_one() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let msg = plan_err(&input(&data, root.path(), &root.path().join("default")));
    assert!(msg.contains("contains DDMM's current data folder"), "{msg}");
}

#[test]
fn rejects_a_folder_inside_or_containing_the_game() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let game = root.path().join("Helldivers 2");
    std::fs::create_dir_all(game.join("data")).unwrap();
    let default_path = root.path().join("default");

    let game_data = game.join("data");
    let mut i = input(&data, &game_data, &default_path);
    i.game_path = Some(&game);
    assert!(plan_err(&i).contains("Helldivers 2 install"));

    let above = root.path().join("games");
    let game_below = above.join("Helldivers 2");
    std::fs::create_dir_all(&game_below).unwrap();
    let mut i = input(&data, &above, &default_path);
    i.game_path = Some(&game_below);
    assert!(plan_err(&i).contains("Helldivers 2 install"));
}

#[test]
fn rejects_a_missing_or_relative_destination() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let default_path = root.path().join("default");
    let missing = root.path().join("nope");
    assert!(plan_err(&input(&data, &missing, &default_path)).contains("doesn't exist"));
    assert!(plan_err(&input(&data, Path::new("relative/dir"), &default_path)).contains("full folder path"));
}

#[test]
fn rejects_when_not_enough_free_space() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let err = plan(&input(&data, &dest, &root.path().join("default")), &|_| Some(1024)).unwrap_err();
    assert!(format!("{err:#}").contains("Not enough free space"), "{err:#}");
    // Enough space is fine, and reported.
    let ok = plan(&input(&data, &dest, &root.path().join("default")), &|_| Some(u64::MAX)).unwrap();
    assert_eq!(ok.free_bytes, Some(u64::MAX));
}

#[cfg(unix)]
#[test]
fn rejects_a_read_only_destination() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("ro");
    std::fs::create_dir(&dest).unwrap();
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores permission bits; nothing to observe then.
    let writable_anyway = crate::data_dir::is_dir_writable(&dest);
    let result = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info);
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !writable_anyway {
        assert!(format!("{:#}", result.unwrap_err()).contains("can't write"));
    }
}

#[test]
fn rejects_a_leftover_unfinished_move() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join(INCOMPLETE_MARKER), b"").unwrap();
    std::fs::write(dest.join("settings.json"), SETTINGS).unwrap();
    // Recognizable settings, but an unfinished move: never adopted.
    assert!(plan_err(&input(&data, &dest, &root.path().join("default"))).contains("unfinished"));
}

#[test]
fn rejects_symlinks_inside_the_data() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&elsewhere, data.join("mods/Linked")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&elsewhere, data.join("mods/Linked")).is_ok();
    if !linked {
        return; // Windows without symlink rights
    }
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    assert!(plan_err(&input(&data, &dest, &root.path().join("default"))).contains("symbolic link"));
}

#[test]
fn unrelated_non_empty_folder_gets_a_ddmm_data_subfolder() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("Documents");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("taxes.pdf"), b"x").unwrap();

    let p = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info).unwrap();
    assert!(p.used_subfolder);
    assert!(!p.existing_data);
    assert_eq!(p.target, dest.join(SUBFOLDER_NAME));
    assert!(p.total_files >= 7, "{p:?}");

    // An existing, unrelated "DDMM Data" with files in it is refused.
    std::fs::create_dir(dest.join(SUBFOLDER_NAME)).unwrap();
    std::fs::write(dest.join(SUBFOLDER_NAME).join("x.txt"), b"x").unwrap();
    assert!(plan_err(&input(&data, &dest, &root.path().join("default"))).contains("already has"));
}

#[test]
fn empty_folder_is_used_directly_even_with_os_clutter() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("Empty");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("Thumbs.db"), b"x").unwrap();
    let p = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info).unwrap();
    assert_eq!(p.target, dest);
    assert!(!p.used_subfolder);
}

#[test]
fn existing_ddmm_data_is_offered_for_adoption_not_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let other = root.path().join("usb");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("profiles.json"), PROFILES).unwrap();

    let p = plan(&input(&data, &other, &root.path().join("default")), &no_free_space_info).unwrap();
    assert!(p.existing_data);
    assert_eq!(p.target, other);
    // And execute refuses to copy over it.
    let err = execute(&p, &REAL_OPS, &mut no_commit, &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("already holds DDMM data"));
    assert_eq!(std::fs::read_to_string(other.join("profiles.json")).unwrap(), PROFILES);

    // Also found as the "DDMM Data" subfolder of a picked parent.
    let parent = root.path().join("drive");
    std::fs::create_dir_all(parent.join(SUBFOLDER_NAME)).unwrap();
    std::fs::write(parent.join("other.txt"), b"x").unwrap();
    std::fs::write(parent.join(SUBFOLDER_NAME).join("settings.json"), SETTINGS).unwrap();
    let p = plan(&input(&data, &parent, &root.path().join("default")), &no_free_space_info).unwrap();
    assert!(p.existing_data && p.used_subfolder);
    assert_eq!(p.target, parent.join(SUBFOLDER_NAME));
}

#[test]
fn a_file_named_settings_json_is_not_ddmm_data() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.json"), br#"{"theme":"dark"}"#).unwrap();
    assert!(!is_ddmm_data_dir(dir.path()));
    std::fs::write(dir.path().join("settings.json"), SETTINGS).unwrap();
    assert!(is_ddmm_data_dir(dir.path()));
}

#[test]
fn reset_refuses_when_already_default_and_when_default_has_partial_data() {
    let root = tempfile::tempdir().unwrap();
    let default_path = root.path().join("default");
    make_data(&default_path);
    let mut i = input(&default_path, &default_path, &default_path);
    i.reset = true;
    assert!(plan_err(&i).contains("already in the default location"));

    let data = root.path().join("custom");
    make_data(&data);
    let clean_default = root.path().join("default2");
    std::fs::create_dir_all(clean_default.join("mods")).unwrap();
    let mut i = input(&data, &clean_default, &clean_default);
    i.reset = true;
    assert!(plan_err(&i).contains("isn't a complete DDMM data folder"));
}

// ---- moving -------------------------------------------------------------

#[test]
fn successful_move_copies_everything_rewrites_paths_and_cleans_up() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    // A downloads folder that lives inside a moved item gets rewritten; the
    // game path outside the data folder doesn't.
    let settings = serde_json::json!({
        "Version": "V1",
        "GamePath": root.path().join("game").to_string_lossy(),
        "SkipList": [],
        "DownloadsPath": data.join("mods").join("incoming").to_string_lossy(),
    });
    std::fs::write(data.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
    // Something that isn't DDMM's (a portable copy's exe, or the webview's
    // cache) stays put.
    std::fs::write(data.join("ddmm.exe"), b"MZ").unwrap();
    let default_path = data.clone();
    let before = snapshot(&data);

    let dest = root.path().join("new");
    std::fs::create_dir(&dest).unwrap();
    let p = plan(&input(&data, &dest, &default_path), &no_free_space_info).unwrap();
    let mut committed = None;
    let mut phases = Vec::new();
    execute(
        &p,
        &REAL_OPS,
        &mut |t: &Path| {
            committed = Some(t.to_path_buf());
            Ok(())
        },
        &mut |pr: &MoveProgress| phases.push(pr.phase),
    )
    .unwrap();
    assert_eq!(committed.as_deref(), Some(dest.as_path()));
    assert!(phases.contains(&"copying") && phases.contains(&"verifying") && phases.contains(&"finishing"));

    let after = snapshot(&dest);
    for (rel, bytes) in &before {
        let top = rel.components().next().unwrap().as_os_str().to_str().unwrap();
        if rel == Path::new("settings.json") {
            continue;
        }
        if MOVED_ITEMS.contains(&top) {
            assert_eq!(after.get(rel), Some(bytes), "{rel:?}");
        } else {
            assert!(!after.contains_key(rel), "{rel:?} shouldn't have been copied");
        }
    }
    assert!(!dest.join(INCOMPLETE_MARKER).exists());
    assert!(std::fs::read_dir(&dest).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(TEMP_PREFIX)));

    let moved: serde_json::Value = serde_json::from_slice(&std::fs::read(dest.join("settings.json")).unwrap()).unwrap();
    let moved_downloads = PathBuf::from(moved["DownloadsPath"].as_str().unwrap());
    assert!(moved_downloads.ends_with(Path::new("mods").join("incoming")));
    assert!(moved_downloads.starts_with(crate::fs_util::resolve_path(&dest).unwrap()) || moved_downloads.starts_with(&dest));
    assert_eq!(moved["GamePath"], settings["GamePath"]);

    // The old data is only removed by cleanup_old, after the commit.
    assert!(data.join("mods/ModA/manifest.json").exists());
    let left = cleanup_old(&p, &default_path);
    assert!(left.is_empty(), "{left:?}");
    for name in MOVED_ITEMS.iter().chain(REGENERATED_ITEMS) {
        assert!(!data.join(name).exists(), "{name} should be gone from the old folder");
    }
    assert!(data.join("ddmm.exe").exists(), "non-DDMM files stay");
    assert!(!data.join("bridge.json").exists(), "the old bridge.json goes too; the restarted app writes a new one");
}

#[test]
fn old_custom_folder_is_removed_when_empty_but_the_default_never_is() {
    let root = tempfile::tempdir().unwrap();
    let custom = root.path().join("custom");
    make_data(&custom);
    let default_path = root.path().join("default");
    std::fs::create_dir(&default_path).unwrap();
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let p = plan(&input(&custom, &dest, &default_path), &no_free_space_info).unwrap();
    execute(&p, &REAL_OPS, &mut no_commit, &mut |_| {}).unwrap();
    cleanup_old(&p, &default_path);
    assert!(!custom.exists());

    // Back to the (empty) default: reset.
    let mut i = input(&dest, &default_path, &default_path);
    i.reset = true;
    let p = plan(&i, &no_free_space_info).unwrap();
    assert!(p.is_reset && p.target == default_path);
    execute(&p, &REAL_OPS, &mut no_commit, &mut |_| {}).unwrap();
    cleanup_old(&p, &default_path);
    assert!(default_path.join("mods/ModA/manifest.json").is_file());
    assert!(!dest.exists());
}

#[test]
fn reset_sets_a_stale_default_logs_folder_aside() {
    let root = tempfile::tempdir().unwrap();
    let custom = root.path().join("custom");
    make_data(&custom);
    let default_path = root.path().join("default");
    std::fs::create_dir_all(default_path.join("logs")).unwrap();
    std::fs::write(default_path.join("logs/old.log"), b"old").unwrap();
    let mut i = input(&custom, &default_path, &default_path);
    i.reset = true;
    let p = plan(&i, &no_free_space_info).unwrap();
    execute(&p, &REAL_OPS, &mut no_commit, &mut |_| {}).unwrap();
    assert!(default_path.join("logs/ddmm.log").is_file());
    assert!(!default_path.join("logs/old.log").exists());
}

fn assert_failed_cleanly(data: &Path, before: &BTreeMap<PathBuf, Vec<u8>>, dest: &Path, dest_existed: bool) {
    assert_eq!(&snapshot(data), before, "old data must be untouched");
    if dest_existed {
        assert_eq!(std::fs::read_dir(dest).unwrap().count(), 0, "no partial copy may be left in {dest:?}");
    } else {
        assert!(!dest.exists(), "{dest:?} must be removed again");
    }
}

#[test]
fn failure_mid_copy_leaves_old_data_and_no_partial_destination() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let before = snapshot(&data);
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let p = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info).unwrap();

    let calls = AtomicUsize::new(0);
    let failing_copy = |a: &Path, b: &Path| {
        if calls.fetch_add(1, Ordering::SeqCst) == 3 {
            Err(io::Error::other("disk yanked"))
        } else {
            std::fs::copy(a, b)
        }
    };
    let ops = MoveOps { copy_file: &failing_copy, rename: &|a, b| std::fs::rename(a, b) };
    let mut committed = false;
    let err = execute(
        &p,
        &ops,
        &mut |_: &Path| {
            committed = true;
            Ok(())
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("disk yanked"), "{err:#}");
    assert!(!committed, "the pointer must not be written");
    assert_failed_cleanly(&data, &before, &dest, true);
}

#[test]
fn failure_while_putting_items_in_place_or_committing_rolls_back() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let before = snapshot(&data);

    // Rename fails on the third item, after two were already placed. The
    // subfolder DDMM created for an unrelated folder is removed again.
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("unrelated.txt"), b"keep me").unwrap();
    let p = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info).unwrap();
    assert!(p.used_subfolder);
    let calls = AtomicUsize::new(0);
    let failing_rename = |a: &Path, b: &Path| {
        if calls.fetch_add(1, Ordering::SeqCst) == 2 {
            Err(io::Error::other("access denied"))
        } else {
            std::fs::rename(a, b)
        }
    };
    let ops = MoveOps { copy_file: &|a, b| std::fs::copy(a, b), rename: &failing_rename };
    assert!(execute(&p, &ops, &mut no_commit, &mut |_| {}).is_err());
    assert_failed_cleanly(&data, &before, &p.target, false);
    assert_eq!(std::fs::read(dest.join("unrelated.txt")).unwrap(), b"keep me");

    // The commit (pointer write) itself fails.
    let dest2 = root.path().join("dest2");
    std::fs::create_dir(&dest2).unwrap();
    let p = plan(&input(&data, &dest2, &root.path().join("default")), &no_free_space_info).unwrap();
    let err = execute(&p, &REAL_OPS, &mut |_: &Path| anyhow::bail!("read-only pointer"), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("read-only pointer"));
    assert_failed_cleanly(&data, &before, &dest2, true);
}

#[test]
fn a_file_changing_during_the_copy_fails_verification() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let p = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info).unwrap();
    // Simulates a file growing between the scan and the copy.
    let growing_copy = |a: &Path, b: &Path| {
        if a.ends_with("profiles.json") {
            std::fs::write(b, format!("{PROFILES} "))?;
            Ok(PROFILES.len() as u64 + 1)
        } else {
            std::fs::copy(a, b)
        }
    };
    let ops = MoveOps { copy_file: &growing_copy, rename: &|a, b| std::fs::rename(a, b) };
    let err = execute(&p, &ops, &mut no_commit, &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("changed while it was being moved"), "{err:#}");
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);
}

#[test]
fn cross_device_rename_falls_back_to_copy() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    make_data(&data);
    let dest = root.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let p = plan(&input(&data, &dest, &root.path().join("default")), &no_free_space_info).unwrap();
    let ops = MoveOps { copy_file: &|a, b| std::fs::copy(a, b), rename: &|_, _| Err(exdev()) };
    execute(&p, &ops, &mut no_commit, &mut |_| {}).unwrap();
    assert_eq!(
        std::fs::read(dest.join("mods/ModA/Options/Red/0123456789abcdef.patch_0")).unwrap(),
        vec![7u8; 4096]
    );
    assert!(std::fs::read_dir(&dest).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(TEMP_PREFIX)));
}

/// A real move across filesystems (the temp dir and the `/dev/shm` tmpfs),
/// when this machine has them on different devices.
#[cfg(target_os = "linux")]
#[test]
fn real_cross_filesystem_move() {
    use std::os::unix::fs::MetadataExt;
    let shm = Path::new("/dev/shm");
    let (Ok(b), true) = (tempfile::tempdir_in(shm), shm.is_dir()) else { return };
    let a = tempfile::tempdir().unwrap();
    if std::fs::metadata(a.path()).unwrap().dev() == std::fs::metadata(b.path()).unwrap().dev() {
        return;
    }
    let data = a.path().join("data");
    make_data(&data);
    let p = plan(&input(&data, b.path(), &a.path().join("default")), &no_free_space_info).unwrap();
    execute(&p, &REAL_OPS, &mut no_commit, &mut |_| {}).unwrap();
    cleanup_old(&p, &a.path().join("default"));
    assert!(b.path().join("mods/ModA/manifest.json").is_file());
    assert!(!data.join("mods").exists());
}

#[test]
fn leftovers_are_recorded_and_retried_safely() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("new");
    std::fs::create_dir(&base).unwrap();
    let old = root.path().join("old");
    std::fs::create_dir_all(old.join("logs")).unwrap();
    std::fs::write(old.join("logs/ddmm.log"), b"x").unwrap();
    std::fs::write(old.join("important.txt"), b"not DDMM's").unwrap();
    std::fs::create_dir_all(base.join("mods")).unwrap();

    record_pending_cleanup(
        &base,
        &[old.join("logs"), old.join("important.txt"), base.join("mods")],
    );
    run_pending_cleanup(&base);
    assert!(!old.join("logs").exists());
    assert!(old.join("important.txt").exists(), "only DDMM's own item names are ever deleted");
    assert!(base.join("mods").exists(), "never anything in the current data folder");
    assert!(!base.join(CLEANUP_FILE).exists());
}

#[test]
fn rewrite_path_only_touches_paths_inside_moved_items() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    std::fs::create_dir_all(old.join("mods")).unwrap();
    let new = root.path().join("new");
    assert_eq!(rewrite_path(&old.join("mods"), &old, &new), Some(new.join("mods")));
    assert!(rewrite_path(&old.join("mods").join("x"), &old, &new).unwrap().ends_with(Path::new("mods").join("x")));
    assert_eq!(rewrite_path(&old, &old, &new), None, "the data folder itself isn't moved as a whole");
    assert_eq!(rewrite_path(&root.path().join("Downloads"), &old, &new), None);
    assert_eq!(rewrite_path(Path::new(""), &old, &new), None);
}

#[test]
fn moved_items_match_the_names_the_rest_of_ddmm_uses() {
    assert!(MOVED_ITEMS.contains(&crate::commands::mods::MODS_DIRECTORY.trim_end_matches('/')));
    assert!(MOVED_ITEMS.contains(&crate::secrets::API_KEY_SLOT.file_name));
    assert!(MOVED_ITEMS.contains(&crate::secrets::OAUTH_SLOT.file_name));
    assert!(REGENERATED_ITEMS.contains(&crate::download::STAGING_DIRECTORY));
}

#[test]
fn human_bytes_is_readable() {
    assert_eq!(human_bytes(512), "512 bytes");
    assert_eq!(human_bytes(1536), "1.5 KB");
    assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
}
