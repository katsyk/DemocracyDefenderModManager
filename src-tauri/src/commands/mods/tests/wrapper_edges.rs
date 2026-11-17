//! Layouts around a wrapper folder (`My Mod/manifest.json` with loose
//! files, OS files or patch files next to it), and wrapped folders that
//! are added where they are, in the mod storage.

use super::*;

const ROOT_PATCH: &str = "fedcba9876543210.patch_0";

fn mods_root(base: &Path) -> PathBuf {
    base.join(MODS_DIRECTORY)
}

fn put(dir: &Path, entries: &[(String, Vec<u8>)]) {
    for (name, data) in entries {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
}

fn zip_of(src: &Path, file: &str, entries: &[(String, Vec<u8>)]) -> PathBuf {
    let zip = src.join(file);
    write_zip(&zip, entries);
    zip
}

fn loose(entries: &mut Vec<(String, Vec<u8>)>, names: &[&str]) {
    for name in names {
        entries.push((name.to_string(), b"loose".to_vec()));
    }
}

// --- archives ---

/// A readme, a preview image, a shortcut and a licence next to the
/// wrapper folder don't stop unwrapping; they describe the download and
/// aren't installed (`assert_authors_manifest` checks the exact contents).
#[tokio::test]
async fn wrapper_with_a_readme_image_and_shortcut_installs_the_authors_mod() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let mut entries = wrapped_entries("Cool Mod/", "Cool Mod", b"v1");
    loose(&mut entries, &["README.txt", "preview.png", "screenshot.JPG", "Nexus page.url", "LICENSE"]);
    let (m, warning) = install(&state, &zip_of(src.path(), "Cool Mod.zip", &entries)).await.unwrap();
    assert_eq!(warning, None);
    assert_authors_manifest(&m, "Cool Mod");
}

#[tokio::test]
async fn wrapper_with_os_files_at_every_level_installs_the_authors_mod() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let mut entries = wrapped_entries("Download/Cool Mod/", "Cool Mod", b"v1");
    loose(
        &mut entries,
        &[
            "__MACOSX/Download/._Cool Mod",
            ".DS_Store",
            "Thumbs.db",
            "desktop.ini",
            "._Download",
            "Download/.DS_Store",
            "Download/__MACOSX/x",
            "Download/Thumbs.db",
        ],
    );
    let (m, warning) = install(&state, &zip_of(src.path(), "Cool Mod.zip", &entries)).await.unwrap();
    assert_eq!(warning, None);
    assert_authors_manifest(&m, "Cool Mod");
}

/// Three wrapper levels, loose files at each of them.
#[tokio::test]
async fn nested_wrappers_with_loose_files_install_the_authors_mod() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let mut entries = wrapped_entries("Pack/Cool Mod v1/Cool Mod/", "Cool Mod", b"v1");
    loose(&mut entries, &["readme.txt", "Pack/info.url", "Pack/Cool Mod v1/changelog.md"]);
    let (m, _) = install(&state, &zip_of(src.path(), "Cool Mod.zip", &entries)).await.unwrap();
    assert_authors_manifest(&m, "Cool Mod");

    // One level deeper than DDMM looks: installed as before.
    let base = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let entries = wrapped_entries("A/B/C/Cool Mod/", "Cool Mod", b"v1");
    let (m, _) = install(&state, &zip_of(src.path(), "Deep.zip", &entries)).await.unwrap();
    assert!(is_local_generated(&m.manifest), "{:?}", m.manifest);
}

/// Patch files next to the wrapper folder: it's unclear which is the mod,
/// so it installs as it always did -- nothing left out -- and says so.
#[tokio::test]
async fn wrapper_next_to_root_patch_files_installs_as_before_with_a_warning() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let mut entries = wrapped_entries("Cool Mod/", "Cool Mod", b"v1");
    entries.push((ROOT_PATCH.into(), b"root".to_vec()));
    entries.push(("readme.txt".into(), b"hi".to_vec()));
    let (m, warning) = install(&state, &zip_of(src.path(), "Cool Mod.zip", &entries)).await.unwrap();

    assert!(is_local_generated(&m.manifest), "{:?}", m.manifest);
    let Manifest::Legacy(legacy) = &m.manifest else { unreachable!() };
    assert_eq!(legacy.options, None, "the top-level patch files are the mod");
    for (name, data) in &entries {
        assert_eq!(&std::fs::read(m.directory.join(name)).unwrap(), data, "{name} kept");
    }
    let warning = warning.expect("a warning");
    assert!(warning.contains("\"Cool Mod\"") && warning.contains("isn't used"), "{warning}");

    // The same from a folder outside the storage.
    let folder = src.path().join("Unpacked");
    put(&folder, &entries);
    let mut guard = state.mods.lock().await;
    let (m, warning) = install_from_folder(base.path(), guard.as_mut().unwrap(), &folder).await.unwrap();
    assert!(is_local_generated(&m.manifest));
    assert!(m.directory.join(ROOT_PATCH).is_file());
    assert!(m.directory.join("Cool Mod").join("Red").join(patch_file_name()).is_file());
    assert!(warning.is_some());
}

/// Plain mods with patch files at the top get no such warning.
#[tokio::test]
async fn root_patch_files_alone_get_no_warning() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let entries = vec![(ROOT_PATCH.to_string(), b"x".to_vec()), ("Extras/readme.txt".to_string(), b"x".to_vec())];
    let (_, warning) = install(&state, &zip_of(src.path(), "Plain.zip", &entries)).await.unwrap();
    assert_eq!(warning, None);
}

// --- adopted in place ---

/// `mods/Cool Mod/Cool Mod v1/Cool Mod/...` with a readme, an image and
/// OS files next to the wrappers, as a user unpacking a download into the
/// storage gets it.
fn wrapped_in_storage(base: &Path) -> PathBuf {
    let dir = mods_root(base).join("Cool Mod");
    put(&dir, &wrapped_entries("Cool Mod v1/Cool Mod/", "Cool Mod", b"v1"));
    let mut extra = Vec::new();
    loose(&mut extra, &["README.txt", ".DS_Store", "__MACOSX/._x", "Cool Mod v1/screenshot.png"]);
    put(&dir, &extra);
    dir
}

async fn adopt(state: &AppState, dir: &Path) -> TAResult<(Mod, Option<String>)> {
    let mut guard = state.mods.lock().await;
    install_from_folder(&state.base_path, guard.as_mut().unwrap(), dir).await
}

/// The mod's root moves up into the folder, which keeps its name: the
/// author's manifest is used, its loose files are kept, nothing is copied.
#[tokio::test]
async fn a_wrapped_folder_in_the_storage_is_adopted_with_its_authors_manifest() {
    let base = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let dir = wrapped_in_storage(base.path());

    let (m, warning) = adopt(&state, &dir).await.unwrap();

    assert_eq!(warning, None);
    assert_eq!(m.directory, dir);
    assert_eq!(m.guid(), Uuid::parse_str(WRAPPED_GUID).unwrap());
    assert!(matches!(m.manifest, Manifest::V1(_)));
    assert_eq!(std::fs::read(dir.join("README.txt")).unwrap(), b"loose", "kept: the user's only copy");
    assert!(dir.join("screenshot.png").is_file());
    assert!(!dir.join("Cool Mod v1").exists());
    assert!(dir.join("Blue").join("Dark").join(patch_file_name()).is_file());
    let (visible, hidden) = visible_and_hidden(&mods_root(base.path()));
    assert_eq!(visible, ["Cool Mod"]);
    assert!(hidden.is_empty(), "{hidden:?}");

    // It loads the same way on the next start.
    let reloaded = load(base.path()).await;
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].guid(), m.guid());
    assert_eq!(reloaded[0].directory, dir);

    // And adding it again is refused without touching it.
    let msg = message(adopt(&state, &dir).await);
    assert!(msg.contains("already installed"), "{msg}");
    assert!(dir.join(MANIFEST_FILE).is_file());
}

/// A folder in the storage that isn't wrapped is adopted exactly as before.
#[tokio::test]
async fn an_unwrapped_folder_in_the_storage_is_adopted_as_before() {
    let base = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let plain = mods_root(base.path()).join("Plain");
    put(&plain, &[(patch_file_name(), b"x".to_vec()), ("readme.txt".into(), b"r".to_vec())]);
    let (m, _) = adopt(&state, &plain).await.unwrap();
    assert!(is_local_generated(&m.manifest));
    let mut names: Vec<String> = std::fs::read_dir(&plain).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, [patch_file_name(), MANIFEST_FILE.to_string(), "readme.txt".to_string()]);

    // A wrapper without a manifest.json: options, nothing moved.
    let wrapped = mods_root(base.path()).join("NoManifest");
    put(&wrapped, &[(format!("Inner/{}", patch_file_name()), b"x".to_vec())]);
    let (m, _) = adopt(&state, &wrapped).await.unwrap();
    let Manifest::Legacy(legacy) = &m.manifest else { panic!("{:?}", m.manifest) };
    assert_eq!(legacy.options.as_deref(), Some(&["Inner".to_string()][..]));
    assert!(wrapped.join("Inner").join(patch_file_name()).is_file());

    // A broken wrapped manifest.json: as before, too.
    let broken = mods_root(base.path()).join("Broken");
    put(&broken, &[("Inner/manifest.json".into(), b"{ broken".to_vec()), (format!("Inner/{}", patch_file_name()), b"x".to_vec())]);
    let (m, _) = adopt(&state, &broken).await.unwrap();
    assert!(is_local_generated(&m.manifest));
    assert!(broken.join("Inner").join(MANIFEST_FILE).is_file());
}

/// Patch files next to the wrapper in the storage: adopted as before,
/// nothing moved, with the warning.
#[tokio::test]
async fn a_folder_in_the_storage_with_root_patch_files_is_adopted_as_before() {
    let base = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let dir = mods_root(base.path()).join("Mixed");
    put(&dir, &wrapped_entries("Cool Mod/", "Cool Mod", b"v1"));
    put(&dir, &[(ROOT_PATCH.into(), b"root".to_vec())]);
    let (m, warning) = adopt(&state, &dir).await.unwrap();
    assert!(is_local_generated(&m.manifest));
    assert!(dir.join("Cool Mod").join(MANIFEST_FILE).is_file());
    assert!(dir.join(ROOT_PATCH).is_file());
    assert!(warning.is_some_and(|w| w.contains("\"Cool Mod\"")));
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.strip_prefix(dir).unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// Refused with nothing changed: a name next to the wrapper that the
/// mod's folder has too, or the author's ID already in the list.
#[tokio::test]
async fn an_adopt_that_cant_unwrap_cleanly_changes_nothing() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let dir = wrapped_in_storage(base.path());
    put(&dir, &[("Cool Mod v1/Cool Mod/readme.txt".into(), b"the mod's own".to_vec())]);
    let before = tree(base.path());
    let msg = message(adopt(&state, &dir).await);
    assert!(msg.contains("README.txt") && msg.contains("Nothing was changed"), "{msg}");
    assert_eq!(tree(base.path()), before);
    assert!(state.mods.lock().await.as_ref().unwrap().is_empty());

    // The same mod is already installed from its archive.
    std::fs::remove_file(dir.join("Cool Mod v1").join("Cool Mod").join("readme.txt")).unwrap();
    install(&state, &zip_of(src.path(), "Cool Mod.zip", &wrapped_entries("Cool Mod/", "Cool Mod", b"v1"))).await.unwrap();
    let before = tree(&dir);
    let msg = message(adopt(&state, &dir).await);
    assert!(msg.contains("already has that ID"), "{msg}");
    assert_eq!(tree(&dir), before);
    assert_eq!(state.mods.lock().await.as_ref().unwrap().len(), 1);
}

/// After adopting, the mod is an ordinary one in its own folder: deleting
/// it removes that folder only, and an update replaces it in place.
#[tokio::test]
async fn an_adopted_wrapped_mod_deletes_and_updates_like_any_other() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = fresh_library(base.path()).await;
    let other = install_v1(&state, src.path(), "A.zip", GUID_A, "Alpha").await;
    let dir = wrapped_in_storage(base.path());
    let (m, _) = adopt(&state, &dir).await.unwrap();

    {
        let update = zip_of(src.path(), "Cool Mod 1.1.zip", &wrapped_entries("Cool Mod 1.1/", "Cool Mod", b"v2"));
        let mut guard = state.mods.lock().await;
        let mods = guard.as_mut().unwrap();
        let (updated, _) = install_update_from_archive(&state, mods, &update, m.guid()).await.unwrap();
        assert_eq!(updated.directory, dir);
        assert_authors_manifest(&updated, "Cool Mod");
        assert_eq!(std::fs::read(dir.join("Red").join(patch_file_name())).unwrap(), b"v2");
        assert_eq!(mods.len(), 2);
    }
    let (visible, hidden) = visible_and_hidden(&mods_root(base.path()));
    assert_eq!(visible.len(), 2);
    assert!(hidden.is_empty(), "{hidden:?}");

    let other_before = tree(&other.directory);
    {
        let mut guard = state.mods.lock().await;
        let cleanup = delete_installed_mod(base.path(), guard.as_mut().unwrap(), m.guid()).await.unwrap();
        cleanup.run().await.unwrap();
    }
    assert!(!dir.exists());
    assert_eq!(tree(&other.directory), other_before);
    let (visible, hidden) = visible_and_hidden(&mods_root(base.path()));
    assert_eq!(visible.len(), 1);
    assert!(hidden.is_empty(), "{hidden:?}");
    assert!(base.path().join(MODS_DIRECTORY).is_dir());
}

/// A crash between the two renames: the next start puts the folder back
/// as it was (it was never set up), and adding it again works.
#[tokio::test]
async fn a_crash_mid_adopt_is_undone_at_startup() {
    let base = tempfile::tempdir().unwrap();
    let dir = wrapped_in_storage(base.path());
    let before = tree(&dir);
    let stage = mods_root(base.path()).join(format!("{}{}", mod_folder::IN_PLACE_UNWRAP_PREFIX, Uuid::new_v4()));
    let mut record = stage.file_name().unwrap().to_os_string();
    record.push(mod_folder::IN_PLACE_UNWRAP_RECORD_SUFFIX);
    std::fs::write(stage.with_file_name(record), br#"{"folder":"Cool Mod","root":["Cool Mod v1","Cool Mod"]}"#).unwrap();
    std::fs::rename(&dir, &stage).unwrap();

    assert!(load(base.path()).await.is_empty());
    assert_eq!(tree(&dir), before);
    let (_, hidden) = visible_and_hidden(&mods_root(base.path()));
    assert!(hidden.is_empty(), "{hidden:?}");

    let state = fresh_library(base.path()).await;
    let (m, _) = adopt(&state, &dir).await.unwrap();
    assert_eq!(m.guid(), Uuid::parse_str(WRAPPED_GUID).unwrap());
}

/// A crash after the mod's root was moved into place: the next start
/// finishes, and the mod loads with its author's manifest.
#[tokio::test]
async fn a_crash_late_in_an_adopt_is_finished_at_startup() {
    let base = tempfile::tempdir().unwrap();
    let dir = wrapped_in_storage(base.path());
    let stage = mods_root(base.path()).join(format!("{}{}", mod_folder::IN_PLACE_UNWRAP_PREFIX, Uuid::new_v4()));
    let mut record = stage.file_name().unwrap().to_os_string();
    record.push(mod_folder::IN_PLACE_UNWRAP_RECORD_SUFFIX);
    std::fs::write(stage.with_file_name(record), br#"{"folder":"Cool Mod","root":["Cool Mod v1","Cool Mod"]}"#).unwrap();
    std::fs::rename(&dir, &stage).unwrap();
    std::fs::rename(stage.join("Cool Mod v1").join("Cool Mod"), &dir).unwrap();

    let loaded = load(base.path()).await;
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].guid(), Uuid::parse_str(WRAPPED_GUID).unwrap());
    assert!(dir.join("README.txt").is_file());
    assert!(dir.join("screenshot.png").is_file());
    let (visible, hidden) = visible_and_hidden(&mods_root(base.path()));
    assert_eq!(visible, ["Cool Mod"]);
    assert!(hidden.is_empty(), "{hidden:?}");
}

/// A mod adopted wrapped before DDMM unwrapped in place has a generated
/// manifest.json at its top: it is left as it is (no automatic move), and
/// an update with its zipped folder gives it the author's manifest and ID.
#[tokio::test]
async fn a_mod_adopted_wrapped_before_is_left_alone_until_updated() {
    let base = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let dir = wrapped_in_storage(base.path());
    let old = Manifest::Legacy(legacy::Manifest {
        guid: Uuid::from_bytes(*b"LOCAL\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b"),
        name: "Cool Mod".into(),
        description: String::new(),
        icon_path: None,
        options: Some(vec!["Cool Mod v1/Cool Mod/Blue".into(), "Cool Mod v1/Cool Mod/Red".into()]),
    });
    std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec(&old).unwrap()).unwrap();
    let before = tree(&dir);

    let loaded = load(base.path()).await;
    assert_eq!(loaded.len(), 1);
    assert!(is_local_generated(&loaded[0].manifest));
    assert_eq!(tree(&dir), before, "not moved on load");

    let state = fresh_library(base.path()).await;
    let update = zip_of(src.path(), "Cool Mod 1.1.zip", &wrapped_entries("Cool Mod 1.1/", "Cool Mod", b"v2"));
    let mut guard = state.mods.lock().await;
    let mods = guard.as_mut().unwrap();
    let (updated, _) = install_update_from_archive(&state, mods, &update, loaded[0].guid()).await.unwrap();
    assert_eq!(updated.directory, dir);
    assert_authors_manifest(&updated, "Cool Mod");
}
