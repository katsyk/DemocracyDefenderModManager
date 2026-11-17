use super::*;
use std::{fs::File, io::Write};

fn make_zip(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join("mod.zip");
    let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
    for (name, data) in entries {
        if let Some(dir) = name.strip_suffix('/') {
            writer.add_directory(dir, zip::write::SimpleFileOptions::default()).unwrap();
        } else {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(data).unwrap();
        }
    }
    writer.finish().unwrap();
    path
}

fn find(entries: &[(&str, &[u8])]) -> Option<(PathBuf, PathBuf)> {
    let tmp = tempfile::tempdir().unwrap();
    let mut archive = Archive::open(&make_zip(tmp.path(), entries)).unwrap();
    find_archive_manifest(&mut archive).unwrap()
}

fn some(root: &str, entry: &str) -> Option<(PathBuf, PathBuf)> {
    Some((PathBuf::from(root), PathBuf::from(entry)))
}

const PATCH: &str = "0123456789abcdef.patch_0";
const MANIFEST: &[u8] = br#"{"Guid":"dddddddd-1111-4111-8111-dddddddddddd","Name":"M","Description":""}"#;

#[test]
fn root_manifest_wins() {
    assert_eq!(
        find(&[("manifest.json", b"{}"), ("Wrapper/manifest.json", b"{}")]),
        some("", "manifest.json")
    );
    assert_eq!(find(&[("Manifest.json", b"{}")]), some("", "Manifest.json"));
}

#[test]
fn single_wrapper_folder_with_manifest_is_the_root() {
    assert_eq!(
        find(&[("ModName/", b""), ("ModName/manifest.json", b"{}"), ("ModName/Red/a", b"")]),
        some("ModName", "ModName/manifest.json")
    );
    // Without a directory entry, and with a case variant.
    assert_eq!(find(&[("ModName/MANIFEST.json", b"{}")]), some("ModName", "ModName/MANIFEST.json"));
}

#[test]
fn nested_wrappers_are_looked_through_a_few_levels() {
    assert_eq!(find(&[("A/B/manifest.json", b"{}")]), some("A/B", "A/B/manifest.json"));
    assert_eq!(find(&[("A/B/C/manifest.json", b"{}")]), some("A/B/C", "A/B/C/manifest.json"));
    assert_eq!(find(&[("A/B/C/D/manifest.json", b"{}")]), None, "deeper than the limit");
}

#[test]
fn os_clutter_and_loose_files_dont_stop_unwrapping() {
    assert_eq!(
        find(&[
            ("__MACOSX/ModName/._manifest.json", b""),
            (".DS_Store", b""),
            ("Thumbs.db", b""),
            ("desktop.ini", b""),
            ("readme.txt", b"hi"),
            ("ModName/.DS_Store", b""),
            ("ModName/manifest.json", b"{}"),
        ]),
        some("ModName", "ModName/manifest.json")
    );
}

#[test]
fn two_top_level_folders_are_not_unwrapped() {
    assert_eq!(find(&[("A/manifest.json", b"{}"), ("B/manifest.json", b"{}")]), None);
    assert_eq!(find(&[("A/manifest.json", b"{}"), ("B/x.txt", b"")]), None);
}

#[test]
fn root_patch_files_mean_the_root_is_the_mod() {
    assert_eq!(find(&[(PATCH, b""), ("Extras/manifest.json", b"{}")]), None);
}

#[test]
fn a_wrapper_without_a_manifest_is_left_alone() {
    let inner = format!("ModName/{PATCH}");
    assert_eq!(find(&[(inner.as_str(), b"")]), None);
}

#[test]
fn backslash_entry_names_count_as_folders() {
    assert_eq!(find(&[("ModName\\manifest.json", b"{}")]), some("ModName", "ModName\\manifest.json"));
}

#[test]
fn folder_root_follows_the_same_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let top = tmp.path().join("Download");
    let inner = top.join("Outer").join("ModName");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::create_dir_all(top.join("__MACOSX")).unwrap();
    std::fs::create_dir_all(top.join(".update-0b7c0f3e-0000-4000-8000-000000000000")).unwrap();
    std::fs::write(top.join(".DS_Store"), b"").unwrap();
    assert_eq!(folder_mod_root(&top), top, "no manifest anywhere: unchanged");
    std::fs::write(inner.join("manifest.json"), b"{ broken").unwrap();
    assert_eq!(folder_mod_root(&top), top, "a broken manifest is ignored");
    std::fs::write(inner.join("manifest.json"), MANIFEST).unwrap();
    assert_eq!(folder_mod_root(&top), inner);
    std::fs::write(top.join("manifest.json"), MANIFEST).unwrap();
    assert_eq!(folder_mod_root(&top), top, "a manifest at the top wins");
    std::fs::remove_file(top.join("manifest.json")).unwrap();
    std::fs::create_dir(top.join("Second")).unwrap();
    assert_eq!(folder_mod_root(&top), top, "two folders: not a wrapper");
}

// --- loose files next to a wrapper folder ---

#[test]
fn a_readme_image_or_shortcut_next_to_the_wrapper_doesnt_stop_unwrapping() {
    for loose in ["README.txt", "readme.md", "preview.png", "cover.JPG", "Nexus page.url", "notes.pdf", "LICENSE"] {
        assert_eq!(
            find(&[(loose, b"x"), ("Mod Name/manifest.json", b"{}"), ("Mod Name/0123456789abcdef.patch_0", b"")]),
            some("Mod Name", "Mod Name/manifest.json"),
            "{loose}"
        );
    }
}

#[test]
fn patch_files_next_to_the_wrapper_keep_the_top_as_the_root() {
    let inner = format!("Mod Name/{PATCH}");
    for root_patch in [PATCH, "0123456789abcdef.patch_0.gpu_resources", "0123456789abcdef.patch_1.stream"] {
        assert_eq!(
            find(&[(root_patch, b""), ("Mod Name/manifest.json", b"{}"), (inner.as_str(), b"")]),
            None,
            "{root_patch}"
        );
    }
}

#[test]
fn nested_wrappers_with_loose_files_at_each_level() {
    assert_eq!(
        find(&[("readme.txt", b""), ("A/info.txt", b""), ("A/B/preview.png", b""), ("A/B/C/manifest.json", b"{}")]),
        some("A/B/C", "A/B/C/manifest.json")
    );
    // Patch files in a middle level: that level is the mod.
    let mid = format!("A/{PATCH}");
    assert_eq!(find(&[(mid.as_str(), b""), ("A/B/manifest.json", b"{}")]), None);
}

fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

#[test]
fn a_wrapped_manifest_next_to_top_patch_files_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let top = tmp.path().join("Mod");
    write(&top.join("Mod Name").join("manifest.json"), MANIFEST);
    assert_eq!(wrapped_manifest_beside_patch_files(&top), None, "no patch files at the top");
    write(&top.join(PATCH), b"");
    write(&top.join("readme.txt"), b"");
    assert_eq!(wrapped_manifest_beside_patch_files(&top).as_deref(), Some("Mod Name"));
    assert_eq!(folder_root_components(&top), None, "the top stays the root");
    write(&top.join("manifest.json"), b"generated");
    assert_eq!(wrapped_manifest_beside_patch_files(&top).as_deref(), Some("Mod Name"), "the top's own manifest is DDMM's");
    write(&top.join("Mod Name").join("manifest.json"), b"{ broken");
    assert_eq!(wrapped_manifest_beside_patch_files(&top), None, "a broken one isn't reported");
    write(&top.join("Mod Name").join("manifest.json"), MANIFEST);
    std::fs::create_dir(top.join("Other")).unwrap();
    assert_eq!(wrapped_manifest_beside_patch_files(&top), None, "two folders");
}

// --- a wrapped folder set up in place ---

/// `mods/Cool Mod/Outer/Cool Mod/{manifest.json, Red/<patch>}` with a
/// readme, an image and OS files next to the wrappers, and another mod
/// next to it. Returns (mods root, mod dir).
fn wrapped_in_storage(tmp: &Path) -> (PathBuf, PathBuf) {
    let mods_root = tmp.join("mods");
    let mod_dir = mods_root.join("Cool Mod");
    let inner = mod_dir.join("Outer").join("Cool Mod");
    write(&inner.join("manifest.json"), MANIFEST);
    write(&inner.join("Red").join(PATCH), b"red");
    write(&inner.join(".DS_Store"), b"inner");
    write(&mod_dir.join("README.txt"), b"read me");
    write(&mod_dir.join(".DS_Store"), b"outer");
    write(&mod_dir.join("__MACOSX").join("._x"), b"");
    write(&mod_dir.join("Outer").join("preview.png"), b"png");
    write(&mods_root.join("Other").join(PATCH), b"other");
    (mods_root, mod_dir)
}

fn sorted_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn root() -> Vec<String> {
    vec!["Outer".to_string(), "Cool Mod".to_string()]
}

fn assert_unwrapped(mods_root: &Path, mod_dir: &Path) {
    assert_eq!(sorted_names(mod_dir), [".DS_Store", "README.txt", "Red", "__MACOSX", "manifest.json", "preview.png"]);
    assert_eq!(std::fs::read(mod_dir.join(".DS_Store")).unwrap(), b"inner", "the mod's own file wins");
    assert_eq!(std::fs::read(mod_dir.join("README.txt")).unwrap(), b"read me");
    assert_eq!(std::fs::read(mod_dir.join("Red").join(PATCH)).unwrap(), b"red");
    assert_eq!(sorted_names(mods_root), ["Cool Mod", "Other"], "no working folder or record left");
    assert_eq!(std::fs::read(mods_root.join("Other").join(PATCH)).unwrap(), b"other");
}

#[test]
fn a_wrapped_folder_is_unwrapped_in_place_keeping_its_loose_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    assert_eq!(folder_root_components(&mod_dir), Some(root()));
    assert_eq!(in_place_clash(&mod_dir, &root()).unwrap(), None, "OS files never clash");
    unwrap_in_place(&mods_root, &mod_dir, &root()).unwrap();
    assert_unwrapped(&mods_root, &mod_dir);
    // Done: the folder is now the mod's root.
    assert_eq!(folder_root_components(&mod_dir), None);
}

#[test]
fn a_name_clash_is_found_before_anything_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, mod_dir) = wrapped_in_storage(tmp.path());
    write(&mod_dir.join("Outer").join("Cool Mod").join("readme.TXT"), b"the mod's");
    assert_eq!(in_place_clash(&mod_dir, &root()).unwrap().as_deref(), Some("README.txt"));
    // Two loose files with one name at different levels clash too.
    std::fs::remove_file(mod_dir.join("Outer").join("Cool Mod").join("readme.TXT")).unwrap();
    write(&mod_dir.join("Outer").join("readme.txt"), b"");
    let clash = in_place_clash(&mod_dir, &root()).unwrap().unwrap();
    assert!(clash.eq_ignore_ascii_case("readme.txt"), "{clash}");
}

#[test]
fn unwrapping_only_touches_a_folder_directly_in_the_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    assert!(unwrap_in_place(&mods_root, &mod_dir.join("Outer"), &["Cool Mod".to_string()]).is_err());
    assert!(unwrap_in_place(&mods_root, &mod_dir, &["..".to_string()]).is_err());
    assert!(unwrap_in_place(&mods_root, &mod_dir, &[]).is_err());
    assert!(mod_dir.join("Outer").join("Cool Mod").join("manifest.json").is_file());
    assert_eq!(sorted_names(&mods_root), ["Cool Mod", "Other"]);
}

#[test]
fn a_missing_mod_root_is_refused_before_anything_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let before = sorted_names(&mod_dir);
    let err = unwrap_in_place(&mods_root, &mod_dir, &["Outer".to_string(), "Missing".to_string()]).unwrap_err();
    assert!(matches!(err, UnwrapFailure::Unchanged(_)), "{err:?}");
    assert!(format!("{err}").contains("Missing"), "{err}");
    assert_eq!(sorted_names(&mods_root), ["Cool Mod", "Other"]);
    assert_eq!(sorted_names(&mod_dir), before);
}

/// The second rename fails (here: the wrapper folder is read-only): the
/// folder is renamed straight back, and the record removed.
#[cfg(unix)]
#[test]
fn a_failed_second_rename_puts_the_folder_back() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let before = sorted_names(&mod_dir);
    let outer = mod_dir.join("Outer");
    std::fs::set_permissions(&outer, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = outer.join("probe");
    if std::fs::write(&probe, b"").is_ok() {
        // Running as root: permissions can't make the rename fail.
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(&outer, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = unwrap_in_place(&mods_root, &mod_dir, &root()).unwrap_err();
    std::fs::set_permissions(&outer, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(err, UnwrapFailure::Unchanged(_)), "{err:?}");
    assert_eq!(sorted_names(&mods_root), ["Cool Mod", "Other"], "renamed back, record removed");
    assert_eq!(sorted_names(&mod_dir), before);
    assert!(mod_dir.join("Outer").join("Cool Mod").join("manifest.json").is_file());
}

/// DDMM stopped right after the folder was set aside: the next start puts
/// it back exactly as it was.
#[test]
fn a_crash_between_the_renames_is_undone_at_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let before = sorted_names(&mod_dir);
    set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    assert!(!mod_dir.exists());
    recover_in_place_unwraps(&mods_root);
    assert_eq!(sorted_names(&mods_root), ["Cool Mod", "Other"]);
    assert_eq!(sorted_names(&mod_dir), before);
    assert!(mod_dir.join("Outer").join("Cool Mod").join("manifest.json").is_file());
    recover_in_place_unwraps(&mods_root);
    assert_eq!(sorted_names(&mods_root), ["Cool Mod", "Other"], "idempotent");
}

/// DDMM stopped after the mod's root was moved into place, partway
/// through moving the rest: the next start finishes and tidies up.
#[test]
fn a_crash_after_the_renames_is_finished_at_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    move_root_into_place(&stage, &mod_dir, &root()).unwrap();
    std::fs::rename(stage.join("README.txt"), mod_dir.join("README.txt")).unwrap();
    recover_in_place_unwraps(&mods_root);
    assert_unwrapped(&mods_root, &mod_dir);
    recover_in_place_unwraps(&mods_root);
    assert_unwrapped(&mods_root, &mod_dir);
}

/// Whatever can't be put in order is left where it is, never deleted.
#[test]
fn recovery_never_deletes_what_it_cant_place() {
    // The mod's folder got a README.txt of its own meanwhile.
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    move_root_into_place(&stage, &mod_dir, &root()).unwrap();
    write(&mod_dir.join("README.txt"), b"new");
    recover_in_place_unwraps(&mods_root);
    assert_eq!(std::fs::read(stage.join("README.txt")).unwrap(), b"read me", "kept in the working folder");
    assert_eq!(std::fs::read(mod_dir.join("README.txt")).unwrap(), b"new");
    assert!(in_place_record_path(&stage).is_file(), "kept for the next start");

    // A folder of the mod's name while its files are still set aside.
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    std::fs::create_dir(&mod_dir).unwrap();
    recover_in_place_unwraps(&mods_root);
    assert!(stage.join("Outer").join("Cool Mod").join("manifest.json").is_file());
    assert!(stage.join("README.txt").is_file());

    // A record that points outside the storage.
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    std::fs::write(in_place_record_path(&stage), br#"{"folder": "..", "root": ["Outer"]}"#).unwrap();
    recover_in_place_unwraps(&mods_root);
    assert!(stage.join("Outer").join("Cool Mod").join("manifest.json").is_file());
    assert!(in_place_record_path(&stage).is_file());
    assert_eq!(sorted_names(tmp.path()), ["mods"]);
}

// --- recovery: what a record may name, and every state it can find ---

#[test]
fn only_plain_names_are_accepted() {
    for ok in ["Cool Mod", ".hidden", "a.b", "x y (2)", "..."] {
        assert!(is_plain_name(ok), "{ok}");
    }
    for bad in ["", ".", "..", "C:", "c:", "Z:", "C:nope", "a:b", "/", "/etc", "a/b", "a\\b", "\\\\server", "a\0b"] {
        assert!(!is_plain_name(bad), "{bad:?}");
    }
}

fn write_record(stage: &Path, json: &[u8]) {
    std::fs::write(in_place_record_path(stage), json).unwrap();
}

/// A record naming a drive (`C:` replaces the whole path when joined on
/// Windows) or anything but plain names is refused on every platform:
/// nothing moves, nothing is deleted, the record stays.
#[test]
fn a_record_with_a_drive_or_path_is_left_alone() {
    for json in [
        br#"{"folder":"Cool","root":["C:","nope"]}"#.as_slice(),
        br#"{"folder":"C:","root":["Outer","Cool Mod"]}"#,
        br#"{"folder":"Cool","root":["Outer","C:\\Windows"]}"#,
        br#"{"folder":"Cool","root":["/tmp"]}"#,
        br#"{"folder":"Cool","root":[]}"#,
        br#"{"folder":".adopt-x","root":["Outer"]}"#,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
        let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
        let before = sorted_names(&stage);
        write_record(&stage, json);
        let left = recover_in_place_unwraps(&mods_root);
        assert_eq!(left, [stage.clone()], "{}", String::from_utf8_lossy(json));
        assert_eq!(sorted_names(&stage), before);
        assert!(in_place_record_path(&stage).is_file());
        assert_eq!(sorted_names(tmp.path()), ["mods"]);
        assert!(!mods_root.join("Cool").exists());
    }
}

#[test]
fn a_garbage_record_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    write_record(&stage, b"\xff not json {");
    assert_eq!(recover_in_place_unwraps(&mods_root), [stage.clone()]);
    assert!(stage.join("Outer").join("Cool Mod").join("manifest.json").is_file());
    assert!(in_place_record_path(&stage).is_file());
    assert!(!mod_dir.exists());
}

/// The record was written, but DDMM stopped before the first rename: the
/// folder is untouched and the record is just removed.
#[test]
fn a_record_without_its_folder_is_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let before = sorted_names(&mod_dir);
    let stage = mods_root.join(format!("{}{}", crate::mod_folder::IN_PLACE_UNWRAP_PREFIX, uuid::Uuid::new_v4()));
    write_record(&stage, br#"{"folder":"Cool Mod","root":["Outer","Cool Mod"]}"#);
    assert!(recover_in_place_unwraps(&mods_root).is_empty());
    assert_eq!(sorted_names(&mods_root), ["Cool Mod", "Other"]);
    assert_eq!(sorted_names(&mod_dir), before);
}

/// The mod's root was moved out, and then the mod's folder went away
/// (deleted, or an update of it stopped halfway): the working folder,
/// now just the wrapper shell, is never renamed into the mod's name.
#[test]
fn a_shell_whose_mod_folder_is_gone_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    move_root_into_place(&stage, &mod_dir, &root()).unwrap();
    std::fs::remove_dir_all(&mod_dir).unwrap();
    assert_eq!(recover_in_place_unwraps(&mods_root), [stage.clone()]);
    assert!(!mod_dir.exists());
    assert!(stage.join("README.txt").is_file());
    assert!(in_place_record_path(&stage).is_file());
}

/// DDMM stopped partway through tidying up: some items moved in, a
/// wrapper folder already removed. The next start finishes.
#[test]
fn a_crash_midway_through_cleanup_is_finished() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    move_root_into_place(&stage, &mod_dir, &root()).unwrap();
    std::fs::rename(stage.join("Outer").join("preview.png"), mod_dir.join("preview.png")).unwrap();
    std::fs::remove_dir(stage.join("Outer")).unwrap();
    std::fs::rename(stage.join("README.txt"), mod_dir.join("README.txt")).unwrap();
    assert!(recover_in_place_unwraps(&mods_root).is_empty());
    assert_unwrapped(&mods_root, &mod_dir);
}

/// A wrapper folder in the working folder that is a symlink is never
/// followed: recovery stops before touching anything, and the folder it
/// points to (outside the storage) keeps its files, OS files included.
#[cfg(unix)]
#[test]
fn a_symlinked_wrapper_is_never_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let outside = tmp.path().join("outside");
    write(&outside.join("secret.txt"), b"keep");
    write(&outside.join(".DS_Store"), b"keep");
    let stage = set_aside_for_unwrap(&mods_root, &mod_dir, &root()).unwrap();
    move_root_into_place(&stage, &mod_dir, &root()).unwrap();
    std::fs::remove_file(stage.join("Outer").join("preview.png")).unwrap();
    std::fs::remove_dir(stage.join("Outer")).unwrap();
    std::os::unix::fs::symlink(&outside, stage.join("Outer")).unwrap();

    assert_eq!(recover_in_place_unwraps(&mods_root), [stage.clone()]);
    assert_eq!(sorted_names(&outside), [".DS_Store", "secret.txt"]);
    assert!(!mod_dir.join("secret.txt").exists());
    assert!(in_place_record_path(&stage).is_file());

    // Nor at adopt time: a symlinked "wrapper" isn't one.
    let top = tmp.path().join("mods").join("Linked");
    std::fs::create_dir_all(&top).unwrap();
    let real = tmp.path().join("real");
    write(&real.join("manifest.json"), MANIFEST);
    std::os::unix::fs::symlink(&real, top.join("Inner")).unwrap();
    assert_eq!(folder_root_components(&top), None);
    assert!(unwrap_in_place(&mods_root, &top, &["Inner".to_string()]).is_err());
    assert!(real.join("manifest.json").is_file());
    assert!(top.join("Inner").is_symlink());
}

/// A symlink next to the wrapper folder is moved as the link itself; what
/// it points to is never touched.
#[cfg(unix)]
#[test]
fn a_symlink_next_to_the_wrapper_moves_as_a_link() {
    let tmp = tempfile::tempdir().unwrap();
    let (mods_root, mod_dir) = wrapped_in_storage(tmp.path());
    let outside = tmp.path().join("outside");
    write(&outside.join("file.txt"), b"keep");
    std::os::unix::fs::symlink(&outside, mod_dir.join("link")).unwrap();
    // An OS-file name that clashes, as a link: only the link is removed.
    std::fs::remove_file(mod_dir.join(".DS_Store")).unwrap();
    std::os::unix::fs::symlink(&outside, mod_dir.join(".DS_Store")).unwrap();
    unwrap_in_place(&mods_root, &mod_dir, &root()).unwrap();
    assert!(mod_dir.join("link").is_symlink());
    assert_eq!(std::fs::read(mod_dir.join(".DS_Store")).unwrap(), b"inner");
    assert_eq!(sorted_names(&outside), ["file.txt"]);
}
