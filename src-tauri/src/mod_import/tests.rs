//! Import tests. Fixtures synthesize the other managers' on-disk layouts as
//! documented in `docs/development/importing-from-other-managers.md`.

use super::*;
use std::io::Write;
use std::sync::atomic::AtomicBool;

const PATCH: &str = "0123456789abcdef.patch_0";

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

/// A tiny but real mod archive: one patch file whose content is unique to
/// `seed`, so every archive hashes differently.
fn mod_zip(path: &Path, seed: &str) {
    make_zip(path, &[(PATCH, seed.as_bytes())]);
}

fn nexus_name(name: &str, id: u32, version: &str, ts: u64) -> String {
    format!("{name}-{id}-{}-{ts}.zip", version.replace('.', "-"))
}

async fn state_with_empty_library(base: &Path) -> AppState {
    let state = AppState::new(base.to_path_buf());
    let mut guard = state.mods.lock().await;
    crate::commands::mods::ensure_mods_loaded(&mut guard, base).await.unwrap();
    drop(guard);
    state
}

async fn installed_index(state: &AppState) -> InstalledIndex {
    let mods = state.mods.lock().await.clone().unwrap_or_default();
    InstalledIndex::build(&mods).await
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// Every file under `dir` with its bytes and modification time, to prove a
/// source was left exactly as it was.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let meta = std::fs::symlink_metadata(e.path()).unwrap();
            if meta.is_dir() {
                out.push((e.path(), Vec::new(), meta.modified().unwrap()));
                stack.push(e.path());
            } else {
                out.push((e.path(), std::fs::read(e.path()).unwrap(), meta.modified().unwrap()));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn installed_dirs(base: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(base.join(MODS_DIRECTORY))
        .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn ids(scan: &ScanResult) -> Vec<ScanItem> {
    scan.items.iter().filter(|i| i.status.selected_by_default()).cloned().collect()
}

#[cfg(unix)]
fn set_tree_readonly(dir: &Path, readonly: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![dir.to_path_buf()];
    let mut all = vec![];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            }
            all.push(e.path());
        }
        all.push(d);
    }
    for p in all {
        let is_dir = p.is_dir();
        let mode = match (is_dir, readonly) {
            (true, true) => 0o555,
            (true, false) => 0o755,
            (false, true) => 0o444,
            (false, false) => 0o644,
        };
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

#[test]
fn nexus_download_names_give_mod_id_version_and_clean_name() {
    let n = nexus_ref_from_name("Better Helmets-1234-1-2-1712345678.zip", true).unwrap();
    assert_eq!(n.mod_id, "1234");
    assert_eq!(n.version.as_deref(), Some("1.2"));
    assert_eq!(n.uploaded_at, Some(1712345678));
    assert_eq!(n.file_name.as_deref(), Some("Better Helmets-1234-1-2-1712345678.zip"));
    assert_eq!(display_name_from_file("Better Helmets-1234-1-2-1712345678.zip", true), "Better Helmets");
    assert_eq!(display_name_from_file("Better Helmets-1234-1-2-1712345678 (1).zip", true), "Better Helmets");

    // An unpacked folder named after the archive (a staging folder).
    let n = nexus_ref_from_name("Better Helmets-1234-1-2-1712345678", false).unwrap();
    assert_eq!(n.mod_id, "1234");
    assert_eq!(n.file_name, None);
    assert_eq!(display_name_from_file("Better Helmets-1234-1-2-1712345678", false), "Better Helmets");

    // Nexus's newer (June 2026) naming.
    let n = nexus_ref_from_name("Better Helmets 1234 1.2 2026-07-01T10-30Z Ab3dE5fG.zip", true).unwrap();
    assert_eq!((n.mod_id.as_str(), n.version.as_deref()), ("1234", Some("1.2")));
    assert_eq!(n.uploaded_at, None, "minute precision never matches Nexus's exact time");
    assert!(n.upload_order > 0);
    assert_eq!(display_name_from_file("Better Helmets 1234 1.2 2026-07-01T10-30Z Ab3dE5fG.zip", true), "Better Helmets");

    assert!(nexus_ref_from_name("my mod v2.zip", true).is_none());
    assert_eq!(display_name_from_file("my mod v2.zip", true), "my mod v2");
    assert_eq!(display_name_from_file("MOD.ZIP", true), "MOD");
}

#[test]
fn safe_dir_names_work_on_every_os() {
    assert_eq!(safe_dir_name("A: B / C?"), "A_ B _ C_");
    assert_eq!(safe_dir_name("trailing. "), "trailing");
    assert_eq!(safe_dir_name("CON"), "mod CON");
    assert_eq!(safe_dir_name("com1.txt"), "mod com1.txt");
    assert_eq!(safe_dir_name(""), "mod");
    assert!(safe_dir_name(&"x".repeat(300)).chars().count() <= 80);
}

#[test]
fn free_space_check() {
    assert!(check_free_space(10, None).is_ok());
    assert!(check_free_space(10, Some(FREE_SPACE_MARGIN + 10)).is_ok());
    let err = check_free_space(10 * 1024 * 1024 * 1024, Some(1024 * 1024 * 1024)).unwrap_err();
    assert!(format!("{err}").contains("Not enough free space"), "{err}");
}

// ---------------------------------------------------------------------------
// Overlap / read-only
// ---------------------------------------------------------------------------

#[test]
fn sources_overlapping_the_data_folder_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    std::fs::create_dir_all(base.join("mods/SomeMod")).unwrap();

    assert!(ensure_source_allowed(&base, &base).is_err());
    assert!(ensure_source_allowed(&base.join("mods"), &base).is_err());
    assert!(ensure_source_allowed(&base.join("mods/SomeMod"), &base).is_err());
    assert!(ensure_source_allowed(root.path(), &base).is_err());
    assert!(ensure_source_allowed(&base.join("mods/../mods"), &base).is_err());
    std::fs::create_dir_all(root.path().join("elsewhere")).unwrap();
    assert!(ensure_source_allowed(&root.path().join("elsewhere"), &base).is_ok());
}

#[cfg(unix)]
#[test]
fn a_source_reached_through_a_symlink_into_the_data_folder_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    std::fs::create_dir_all(base.join("mods")).unwrap();
    std::os::unix::fs::symlink(base.join("mods"), root.path().join("link")).unwrap();
    assert!(ensure_source_allowed(&root.path().join("link"), &base).is_err());
}

#[tokio::test]
async fn an_item_inside_the_data_folder_is_refused_at_import_time() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let state = state_with_empty_library(&base).await;
    // A picked file that lives in DDMM's data folder (scan_paths doesn't
    // check the root, run_import checks each item).
    let inside = base.join("stray.zip");
    mod_zip(&inside, "stray");
    let scan = scan_paths(std::slice::from_ref(&inside), &installed_index(&state).await, &no_cancel(), |_| {});
    let report = run_import(&state, scan.items.clone(), &no_cancel(), |_| {}).await;
    assert!(report.imported.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].reason.contains("data folder"), "{}", report.failed[0].reason);
    assert!(inside.is_file());
}

// ---------------------------------------------------------------------------
// A folder of hundreds of archives
// ---------------------------------------------------------------------------

#[tokio::test]
async fn imports_300_archives_from_a_read_only_downloads_folder_with_nexus_metadata() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let downloads = root.path().join("Downloads");
    std::fs::create_dir_all(downloads.join("hd2/armor")).unwrap();
    for i in 0..300u32 {
        // Some in a nested subfolder, like a sorted Downloads folder.
        let dir = if i % 10 == 0 { downloads.join("hd2/armor") } else { downloads.clone() };
        mod_zip(&dir.join(nexus_name(&format!("Mod {i}"), 1000 + i, "1.0", 1_712_000_000 + i as u64)), &format!("mod {i}"));
    }
    // Not mods: skipped, not imported.
    make_zip(&downloads.join("holiday photos.zip"), &[("IMG_0001.jpg", b"jpeg")]);
    std::fs::write(downloads.join("setup.exe"), b"MZ").unwrap();
    std::fs::write(downloads.join("still downloading.zip.crdownload"), b"").unwrap();

    #[cfg(unix)]
    set_tree_readonly(&downloads, true);
    let before = snapshot(&downloads);

    let state = state_with_empty_library(&base).await;
    let mut progress_calls = 0;
    let scan = scan_folder(&downloads, &installed_index(&state).await, &no_cancel(), |_| progress_calls += 1).unwrap();
    assert_eq!(progress_calls, 301);
    assert_eq!(scan.items.len(), 301);
    assert_eq!(scan.items.iter().filter(|i| i.status == ItemStatus::New).count(), 300);
    assert_eq!(scan.items.iter().filter(|i| i.status == ItemStatus::NotAMod).count(), 1);

    let mut last = None;
    let report = run_import(&state, ids(&scan), &no_cancel(), |p| last = Some(p)).await;
    assert_eq!(report.imported.len(), 300, "{:?}", report.failed);
    assert!(report.failed.is_empty());
    assert!(!report.cancelled);
    let last = last.unwrap();
    assert_eq!((last.done, last.total), (300, 300));
    assert_eq!(last.bytes_done, last.bytes_total);

    assert_eq!(installed_dirs(&base).len(), 300);
    let mods = state.mods.lock().await.clone().unwrap();
    assert_eq!(mods.len(), 300);
    let m = mods.iter().find(|m| m.name() == "Mod 42").expect("clean name, no Nexus suffix");
    assert!(m.directory.join(PATCH).is_file());
    // Update checks know the Nexus page, version and exact file right away.
    let sidecar = sources::load_origin_sidecar(&m.directory).await.unwrap();
    assert_eq!(sidecar.sources.len(), 1);
    assert_eq!(sidecar.sources[0].provider, "nexus");
    assert_eq!(sidecar.sources[0].id.as_deref(), Some("1042"));
    assert_eq!(sidecar.sources[0].version.as_deref(), Some("1.0"));
    assert_eq!(sidecar.installed_files[0].file_name.as_deref(), Some("Mod 42-1042-1-0-1712000042.zip"));
    assert_eq!(sidecar.installed_files[0].uploaded_at, Some(1712000042));
    assert!(sidecar.imported_archive.is_some());
    assert!(m.sources.iter().any(|s| s.page_url.as_deref() == Some("https://www.nexusmods.com/helldivers2/mods/1042")));

    // The source is exactly as it was.
    assert_eq!(snapshot(&downloads), before);
    #[cfg(unix)]
    set_tree_readonly(&downloads, false);

    // Scanning again: everything is recognized as already in DDMM.
    let rescan = scan_folder(&downloads, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert_eq!(rescan.items.iter().filter(|i| matches!(i.status, ItemStatus::Installed { .. })).count(), 300);
    assert!(ids(&rescan).is_empty());
}

#[tokio::test]
async fn duplicates_older_versions_and_broken_archives_are_flagged_and_do_not_stop_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let src = root.path().join("archives");
    std::fs::create_dir_all(&src).unwrap();

    mod_zip(&src.join("Cool Armor.zip"), "cool");
    std::fs::copy(src.join("Cool Armor.zip"), src.join("Cool Armor (1).zip")).unwrap();
    // Two downloads of the same Nexus file, and a different file of the
    // same Nexus page (an optional variant) that must not count as "older".
    mod_zip(&src.join(nexus_name("Capes", 77, "1.0", 1_700_000_000)), "capes v1");
    mod_zip(&src.join(nexus_name("Capes", 77, "1.1", 1_710_000_000)), "capes v1.1");
    mod_zip(&src.join(nexus_name("Capes Red Variant", 77, "1.1", 1_710_000_001)), "capes red");
    // Broken ones.
    std::fs::write(src.join("truncated.zip"), b"PK\x03\x04 this is not really a zip").unwrap();
    make_zip(&src.join("evil.zip"), &[("../../escape.patch_0", b"x"), (PATCH, b"y")]);
    make_zip(&src.join("bad manifest.zip"), &[("manifest.json", b"{ nope"), (PATCH, b"z")]);
    // Author manifest with its own GUID, twice (renamed copies).
    let manifest = br#"{"Version":1,"Guid":"11111111-2222-3333-4444-555555555555","Name":"Author Mod","Description":"","IconPath":null,"Options":null}"#;
    make_zip(&src.join("author-a.zip"), &[("manifest.json", manifest), (PATCH, b"a")]);
    make_zip(&src.join("author-b.zip"), &[("manifest.json", manifest), (PATCH, b"b")]);

    let state = state_with_empty_library(&base).await;
    let scan = scan_folder(&src, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    let status = |file: &str| scan.items.iter().find(|i| i.path.file_name().unwrap() == file).unwrap().status.clone();

    assert_eq!(status("Cool Armor.zip"), ItemStatus::New);
    assert!(matches!(status("Cool Armor (1).zip"), ItemStatus::Duplicate { .. }));
    assert!(matches!(status("Capes-77-1-0-1700000000.zip"), ItemStatus::OlderVersion { .. }));
    assert_eq!(status("Capes-77-1-1-1710000000.zip"), ItemStatus::New);
    assert_eq!(status("Capes Red Variant-77-1-1-1710000001.zip"), ItemStatus::New);
    assert!(matches!(status("truncated.zip"), ItemStatus::Unreadable { .. }));
    assert!(matches!(status("evil.zip"), ItemStatus::Unreadable { reason } if reason.contains("unsafe")));
    assert!(matches!(status("bad manifest.zip"), ItemStatus::Unreadable { reason } if reason.contains("JSON")));
    assert_eq!(status("author-a.zip"), ItemStatus::New);
    assert!(matches!(status("author-b.zip"), ItemStatus::Duplicate { .. }));
    assert!(status("truncated.zip").blocked());
    assert!(!ItemStatus::OlderVersion { of: String::new() }.blocked());

    // Importing everything that isn't blocked -- including the older
    // version, and a zip whose data is damaged past its index -- keeps
    // going past the failure.
    let damaged = src.join("damaged data.zip");
    {
        // Stored (uncompressed), so the payload can be found and damaged.
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&damaged).unwrap());
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(PATCH, stored).unwrap();
        writer.write_all(b"damaged-data-payload-damaged-data-payload-damaged-data-payload").unwrap();
        writer.finish().unwrap();
    }
    let mut bytes = std::fs::read(&damaged).unwrap();
    let pos = bytes.windows(7).position(|w| w == b"damaged").unwrap();
    bytes[pos + 40] ^= 0xff; // flip a byte of the stored data: CRC mismatch on extract
    std::fs::write(&damaged, bytes).unwrap();

    let scan = scan_folder(&src, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    let chosen: Vec<ScanItem> = scan.items.iter().filter(|i| !i.status.blocked() && i.status != ItemStatus::NotAMod).cloned().collect();
    let report = run_import(&state, chosen, &no_cancel(), |_| {}).await;
    let names: Vec<&str> = report.imported.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"Cool Armor"));
    assert!(names.contains(&"Author Mod"));
    assert!(names.contains(&"Capes Red Variant"));
    // Both Capes downloads were chosen; the second gets its own folder
    // (and, having no manifest of its own, that folder's name).
    assert!(names.contains(&"Capes") && names.contains(&"Capes (2)"), "{names:?}");
    assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
    assert_eq!(report.failed[0].name, "damaged data");
    // Distinct folders for the two "Capes", and no leftover of the failure.
    let dirs = installed_dirs(&base);
    assert!(dirs.contains(&"Capes".to_string()) && dirs.contains(&"Capes (2)".to_string()), "{dirs:?}");
    assert!(!dirs.iter().any(|d| d.starts_with("damaged")), "{dirs:?}");
    assert!(!root.path().join("escape.patch_0").exists());
}

#[tokio::test]
async fn cancel_keeps_finished_mods_and_removes_the_one_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let src = root.path().join("archives");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..20 {
        mod_zip(&src.join(format!("mod {i:02}.zip")), &format!("{i}"));
    }
    let state = state_with_empty_library(&base).await;
    let scan = scan_folder(&src, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();

    let cancel = AtomicBool::new(false);
    // Cancel is pressed while the 6th mod (index 5) is being imported.
    let report = run_import(&state, ids(&scan), &cancel, |p| {
        if p.done == 5 && p.current.is_some() {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .await;
    assert!(report.cancelled);
    assert_eq!(report.imported.len(), 5);
    assert_eq!(report.rolled_back.as_deref(), Some("mod 05"));
    assert_eq!(report.not_started, 14);
    assert_eq!(installed_dirs(&base), vec!["mod 00", "mod 01", "mod 02", "mod 03", "mod 04"]);
    assert_eq!(state.mods.lock().await.as_ref().unwrap().len(), 5);

    // Running it again picks up the rest.
    let rescan = scan_folder(&src, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert_eq!(ids(&rescan).len(), 15);
    let report = run_import(&state, ids(&rescan), &no_cancel(), |_| {}).await;
    assert_eq!(report.imported.len(), 15);
    assert_eq!(installed_dirs(&base).len(), 20);
}

#[tokio::test]
async fn a_cancelled_scan_stops_early() {
    let root = tempfile::tempdir().unwrap();
    for i in 0..10 {
        mod_zip(&root.path().join(format!("{i}.zip")), &i.to_string());
    }
    let cancel = AtomicBool::new(false);
    let scan = scan_folder(root.path(), &InstalledIndex::default(), &cancel, |p| {
        if p.done == 3 {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .unwrap();
    assert_eq!(scan.items.len(), 3);
}

#[tokio::test]
async fn archives_added_before_with_add_file_count_as_installed() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let src = root.path().join("dl");
    std::fs::create_dir_all(&src).unwrap();
    mod_zip(&src.join("Old Favourite.zip"), "fav");
    mod_zip(&src.join(nexus_name("Via Browser", 555, "2.0", 1_720_000_000)), "browser");

    let state = state_with_empty_library(&base).await;
    {
        // "Add" of Old Favourite.zip (folder named after the archive).
        let mut guard = state.mods.lock().await;
        crate::commands::mods::install_from_archive(&state, guard.as_mut().unwrap(), &src.join("Old Favourite.zip"))
            .await
            .unwrap();
    }
    {
        // A browser install of another version of the Nexus mod.
        let mut guard = state.mods.lock().await;
        let other = root.path().join(nexus_name("Via Browser", 555, "1.0", 1_710_000_000));
        mod_zip(&other, "browser old");
        let (m, _) = crate::commands::mods::install_from_archive(&state, guard.as_mut().unwrap(), &other).await.unwrap();
        sources::write_origin_sidecar(&m.directory, vec![Source { provider: "nexus".into(), id: Some("555".into()), url: None, version: Some("1.0".into()) }])
            .await
            .unwrap();
        let mods = guard.as_mut().unwrap();
        let entry = mods.iter_mut().find(|x| x.guid() == m.guid()).unwrap();
        entry.resolve_sources().await;
    }
    let scan = scan_folder(&src, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    let status = |file: &str| scan.items.iter().find(|i| i.path.file_name().unwrap() == file).unwrap().status.clone();
    assert!(matches!(status("Old Favourite.zip"), ItemStatus::Installed { .. }));
    assert!(matches!(status("Via Browser-555-2-0-1720000000.zip"), ItemStatus::InstalledOtherVersion { .. }));
}

// ---------------------------------------------------------------------------
// Another manager's folders
// ---------------------------------------------------------------------------

/// Helldivers 2 Mod Manager (and older DDMM) storage: one folder per mod
/// with its manifest.json (a generated `LOCAL` one for mods that had none),
/// and profiles.json with the load order, on/off state and chosen options.
fn hd2mm_storage(root: &Path) -> PathBuf {
    let storage = root.join("Helldivers2ModManager");
    let mods = storage.join("Mods");
    let v1 = mods.join("Fancy Capes");
    std::fs::create_dir_all(v1.join("Red")).unwrap();
    std::fs::create_dir_all(v1.join("Blue")).unwrap();
    std::fs::write(v1.join("Red").join(PATCH), b"red").unwrap();
    std::fs::write(v1.join("Blue").join(PATCH), b"blue").unwrap();
    std::fs::write(
        v1.join("manifest.json"),
        br#"{
  "Version": 1,
  "Guid": "aaaaaaaa-0000-0000-0000-000000000001",
  "Name": "Fancy Capes",
  "Description": "Two colours",
  "IconPath": null,
  "Options": [
    { "Name": "Red", "Description": "", "Include": ["Red"], "Image": null, "SubOptions": null },
    { "Name": "Blue", "Description": "", "Include": ["Blue"], "Image": null, "SubOptions": null }
  ],
  "NexusData": { "ModId": 4321, "Version": "2.0" }
}"#,
    )
    .unwrap();
    // A manifest the manager generated for an archive without one.
    let local = mods.join("loose-mod");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join(PATCH), b"loose").unwrap();
    std::fs::write(
        local.join("manifest.json"),
        br#"{"Guid":"4c4f4341-4c01-0203-0405-060708090a0b","Name":"loose-mod","Description":"","IconPath":null,"Options":null}"#,
    )
    .unwrap();
    std::fs::write(
        local.join(ORIGIN_SIDECAR_FILE),
        br#"{"Sources":[{"Provider":"gamebanana","Id":"999","Version":"3"}],"InstalledAt":1700000000}"#,
    )
    .unwrap();
    // A third mod that is disabled.
    let off = mods.join("Quiet Guns");
    std::fs::create_dir_all(&off).unwrap();
    std::fs::write(off.join(PATCH), b"quiet").unwrap();
    std::fs::write(
        off.join("manifest.json"),
        br#"{"Guid":"aaaaaaaa-0000-0000-0000-000000000003","Name":"Quiet Guns","Description":"","IconPath":null,"Options":null}"#,
    )
    .unwrap();
    std::fs::write(
        storage.join("profiles.json"),
        br#"{
  "Profiles": [
    { "Version": "V1", "Name": "Other", "Configs": [] },
    { "Version": "V1", "Name": "Main", "Configs": [
      { "For": "Legacy", "Guid": "4c4f4341-4c01-0203-0405-060708090a0b", "Enabled": true, "Selected": 0 },
      { "For": "Legacy", "Guid": "aaaaaaaa-0000-0000-0000-000000000003", "Enabled": false, "Selected": 0 },
      { "For": "V1", "Guid": "aaaaaaaa-0000-0000-0000-000000000001", "Enabled": true, "Toggled": [false, true], "Selected": [0, 0] }
    ] }
  ],
  "Active": 1
}"#,
    )
    .unwrap();
    std::fs::write(storage.join("settings.json"), b"{}").unwrap();
    storage
}

#[tokio::test]
async fn imports_another_managers_storage_with_profile_order_state_and_options() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let storage = hd2mm_storage(root.path());
    let before = snapshot(&storage);

    let state = state_with_empty_library(&base).await;
    let dirs = KnownDirs { local_data: Some(root.path().to_path_buf()), ..Default::default() };
    let detected = detect_sources(&dirs, &base);
    assert_eq!(detected.len(), 1, "{detected:?}");
    assert_eq!(detected[0].path, storage.join("Mods"));
    assert_eq!(detected[0].kind, SourceKind::ManagerMods);
    assert_eq!(detected[0].count, 3);

    let scan = scan_folder(&detected[0].path, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert_eq!(scan.profile_name.as_deref(), Some("Main"));
    assert_eq!(scan.items.len(), 3);
    let capes = scan.items.iter().find(|i| i.name == "Fancy Capes").unwrap();
    assert_eq!(capes.kind, ItemKind::Folder);
    assert_eq!(capes.nexus.as_ref().unwrap().mod_id, "4321");
    let hint = capes.profile.as_ref().unwrap();
    assert_eq!(hint.order, 2);
    assert!(hint.enabled);
    assert_eq!(hint.toggled, Some(vec![false, true]));
    let quiet = scan.items.iter().find(|i| i.name == "Quiet Guns").unwrap();
    assert!(!quiet.profile.as_ref().unwrap().enabled);

    let report = run_import(&state, ids(&scan), &no_cancel(), |_| {}).await;
    assert_eq!(report.imported.len(), 3, "{:?}", report.failed);
    // GUIDs are kept, so the profile configs apply as they are.
    let capes = report.imported.iter().find(|m| m.name == "Fancy Capes").unwrap();
    assert_eq!(capes.guid.to_string(), "aaaaaaaa-0000-0000-0000-000000000001");
    assert_eq!(capes.order, Some(2));
    assert!(matches!(&capes.config, Some(Config::V1 { guid, enabled: true, toggled, selected })
        if guid == &capes.guid && toggled == &vec![false, true] && selected == &vec![0, 0]));
    let quiet = report.imported.iter().find(|m| m.name == "Quiet Guns").unwrap();
    assert_eq!(quiet.enabled, Some(false));
    assert!(matches!(&quiet.config, Some(Config::Legacy { enabled: false, .. })));
    let loose = report.imported.iter().find(|m| m.name == "loose-mod").unwrap();
    assert_eq!(loose.guid.to_string(), "4c4f4341-4c01-0203-0405-060708090a0b");

    let mods = state.mods.lock().await.clone().unwrap();
    let loose_mod = mods.iter().find(|m| m.name() == "loose-mod").unwrap();
    // The source's recorded origin came along.
    assert!(loose_mod.sources.iter().any(|s| s.provider == "gamebanana"));
    let capes_mod = mods.iter().find(|m| m.name() == "Fancy Capes").unwrap();
    assert!(capes_mod.directory.join("Red").join(PATCH).is_file());
    // Declared by the manifest already: shown once, not twice.
    assert_eq!(capes_mod.sources.iter().filter(|s| s.provider == "nexus").count(), 1);

    assert_eq!(snapshot(&storage), before);

    // A second scan knows they're all here.
    let rescan = scan_folder(&storage.join("Mods"), &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert!(rescan.items.iter().all(|i| matches!(i.status, ItemStatus::Installed { .. })), "{:?}", rescan.items);
}

/// Vortex: a staging folder with one unpacked folder per mod, named after
/// the Nexus archive, plus its marker file; and its download folder of the
/// original archives.
#[tokio::test]
async fn imports_a_staging_folder_and_finds_manager_downloads() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let appdata = root.path().join("Roaming");
    let staging = appdata.join("Vortex/helldivers2/mods");
    let downloads = appdata.join("Vortex/downloads/helldivers2");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("__vortex_staging_folder"), b"{\"instance\":\"x\",\"game\":\"helldivers2\"}").unwrap();
    for (name, id) in [("Super Earth Flags", 12u32), ("Loud Autocannon", 34)] {
        let folder = staging.join(nexus_name(name, id, "1.0", 1_715_000_000).trim_end_matches(".zip"));
        std::fs::create_dir_all(folder.join("data")).unwrap();
        std::fs::write(folder.join("data").join(PATCH), name.as_bytes()).unwrap();
        mod_zip(&downloads.join(nexus_name(name, id, "1.0", 1_715_000_000)), name);
    }

    let state = state_with_empty_library(&base).await;
    let detected = detect_sources(&KnownDirs { config: Some(appdata.clone()), ..Default::default() }, &base);
    let kinds: Vec<_> = detected.iter().map(|d| (d.kind, d.count)).collect();
    assert_eq!(kinds, vec![(SourceKind::ManagerMods, 2), (SourceKind::ManagerDownloads, 2)]);

    let scan = scan_folder(&staging, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert_eq!(scan.items.len(), 2);
    let flags = scan.items.iter().find(|i| i.name == "Super Earth Flags").unwrap();
    assert_eq!(flags.nexus.as_ref().unwrap().mod_id, "12");
    let report = run_import(&state, ids(&scan), &no_cancel(), |_| {}).await;
    assert_eq!(report.imported.len(), 2);
    let mods = state.mods.lock().await.clone().unwrap();
    let flags = mods.iter().find(|m| m.name() == "Super Earth Flags").unwrap();
    // Patch files one level down: turned into an option like any install.
    assert!(flags.directory.join("data").join(PATCH).is_file());
    let sidecar = sources::load_origin_sidecar(&flags.directory).await.unwrap();
    assert_eq!(sidecar.sources[0].id.as_deref(), Some("12"));
    assert_eq!(sidecar.sources[0].version.as_deref(), Some("1.0"));

    // The same mods in the download folder are recognized as installed.
    let dl_scan = scan_folder(&downloads, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert!(dl_scan.items.iter().all(|i| matches!(i.status, ItemStatus::Installed { .. })), "{:?}", dl_scan.items);
}

#[test]
fn detection_skips_missing_empty_and_overlapping_folders() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    std::fs::create_dir_all(base.join("mods")).unwrap();
    let empty_downloads = root.path().join("Downloads");
    std::fs::create_dir_all(&empty_downloads).unwrap();
    std::fs::write(empty_downloads.join("notes.txt"), b"hi").unwrap();

    let dirs = KnownDirs {
        config: Some(root.path().join("nope")),
        local_data: Some(root.path().join("nope2")),
        downloads: Some(empty_downloads.clone()),
        // A downloads folder set to DDMM's own data folder: never offered.
        settings_downloads: Some(base.clone()),
    };
    mod_zip(&base.join("mods").join("x.zip"), "x");
    assert!(detect_sources(&dirs, &base).is_empty());

    mod_zip(&empty_downloads.join("a.zip"), "a");
    let found = detect_sources(&dirs, &base);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, SourceKind::Downloads);
}

#[tokio::test]
async fn picked_files_scan_like_a_folder() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("A.zip");
    let b = root.path().join("B.7z");
    mod_zip(&a, "a");
    std::fs::write(&b, b"not a 7z").unwrap();
    let folder = root.path().join("Loose");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(PATCH), b"l").unwrap();
    let scan = scan_paths(&[a, b, folder], &InstalledIndex::default(), &no_cancel(), |_| {});
    assert_eq!(scan.items.len(), 3);
    assert_eq!(scan.items[0].status, ItemStatus::New);
    assert!(matches!(scan.items[1].status, ItemStatus::Unreadable { .. }));
    assert_eq!(scan.items[2].kind, ItemKind::Folder);
    assert_eq!(scan.items[2].status, ItemStatus::New);
}

// ---------------------------------------------------------------------------
// More managers
// ---------------------------------------------------------------------------

/// HD2 Arsenal (0.36.x): `hd2a_data.json` in `%LOCALAPPDATA%\hd2arsenal`
/// with the library (paths, labels, options, nexusData) and per-profile
/// lists (order, enabled, optionsConfig by option name); mods unpacked in
/// `mods\<name>`.
fn arsenal_data_folder(local: &Path) -> PathBuf {
    let data = local.join("hd2arsenal");
    let mods = data.join("mods");
    // A Nexus mod without a manifest, with two options (variant folders).
    let helmets = mods.join("Better Helmets");
    std::fs::create_dir_all(helmets.join("Classic")).unwrap();
    std::fs::create_dir_all(helmets.join("Shiny").join("Gold")).unwrap();
    std::fs::create_dir_all(helmets.join("Shiny").join("Silver")).unwrap();
    std::fs::write(helmets.join("Classic").join(PATCH), b"classic").unwrap();
    std::fs::write(helmets.join("Shiny/Gold").join(PATCH), b"gold").unwrap();
    std::fs::write(helmets.join("Shiny/Silver").join(PATCH), b"silver").unwrap();
    // A local mod with flat patch files, disabled.
    let flat = mods.join("my_local_mod");
    std::fs::create_dir_all(&flat).unwrap();
    std::fs::write(flat.join(PATCH), b"flat").unwrap();
    // A mod whose folder is gone (Arsenal still lists it).
    let helmets_path = helmets.to_string_lossy().replace('\\', "\\\\");
    let flat_path = flat.to_string_lossy().replace('\\', "\\\\");
    let gone_path = mods.join("deleted").to_string_lossy().replace('\\', "\\\\");
    std::fs::create_dir_all(data.join("temp")).unwrap();
    mod_zip(&data.join("temp").join("Better Helmets-4242-2-1-1719000000.zip"), "temp archive");
    std::fs::write(
        data.join(layouts::ARSENAL_DATA_FILE),
        format!(
            r#"{{
  "librarySystemVersion": 1,
  "selectedProfile": "main",
  "profileOrder": ["default", "main"],
  "setTopPriority": false,
  "modsLibrary": [
    {{ "uuid": "0b8f3c1e-8a41-4b52-9d2c-3f1e2d4c5b6a", "path": "{helmets_path}", "label": "Better Helmets",
       "description": "Helmets, but better", "iconPath": null, "tags": ["armor"],
       "nexusData": {{ "modId": "4242", "fileId": "17001", "version": "2.1", "updateTimestamp": 1719000000 }},
       "options": [
         {{ "name": "Classic", "description": "", "include": ["Classic"], "enabled": true, "iconPath": null, "suboptions": [] }},
         {{ "name": "Shiny", "description": "", "include": [], "enabled": false, "iconPath": null, "suboptions": [
            {{ "name": "Gold", "description": "", "include": ["Shiny/Gold"], "enabled": true, "iconPath": null }},
            {{ "name": "Silver", "description": "", "include": ["Shiny/Silver"], "enabled": false, "iconPath": null }} ] }}
       ], "contentHash": "abc", "addedAt": "2026-06-01T00:00:00Z" }},
    {{ "uuid": "5d6e7f80-1111-4222-8333-944455566677", "path": "{flat_path}", "label": "My Local Mod",
       "description": "", "iconPath": null, "tags": ["misc"], "nexusData": null, "options": [] }},
    {{ "uuid": "99999999-1111-4222-8333-944455566677", "path": "{gone_path}", "label": "Deleted", "options": [] }}
  ],
  "modsList": {{
    "default": {{ "label": "Default", "mods": [] }},
    "main": {{ "label": "Main loadout", "mods": [
      {{ "uuid": "5d6e7f80-1111-4222-8333-944455566677", "enabled": false, "deployed": false, "optionsConfig": [] }},
      {{ "type": "separator", "label": "Armor" }},
      {{ "uuid": "0b8f3c1e-8a41-4b52-9d2c-3f1e2d4c5b6a", "enabled": true, "deployed": true,
         "optionsConfig": [ {{ "name": "Classic", "enabled": false }},
                            {{ "name": "Shiny", "enabled": true, "suboptions": [ {{ "name": "Gold", "enabled": false }}, {{ "name": "Silver", "enabled": true }} ] }} ] }}
    ] }}
  }}
}}"#
        ),
    )
    .unwrap();
    data
}

#[tokio::test]
async fn imports_an_arsenal_library_with_names_nexus_ids_order_and_options() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("ddmm");
    let local = root.path().join("Local");
    let data = arsenal_data_folder(&local);
    let before = snapshot(&data);

    let state = state_with_empty_library(&base).await;
    let detected = detect_sources(&KnownDirs { local_data: Some(local.clone()), ..Default::default() }, &base);
    assert_eq!(detected.len(), 1, "{detected:?}");
    assert_eq!(detected[0].path, data);
    assert_eq!(detected[0].count, 2, "the listed mod whose folder is gone isn't counted");

    for root_picked in [data.clone(), data.join("mods")] {
        let scan = scan_folder(&root_picked, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
        assert_eq!(scan.items.len(), 2, "library entries only; temp archives are Arsenal's own: {:?}", scan.items);
        assert_eq!(scan.profile_name.as_deref(), Some("Main loadout"));
    }
    let scan = scan_folder(&data, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    let helmets = scan.items.iter().find(|i| i.name == "Better Helmets").unwrap();
    let nexus = helmets.nexus.as_ref().unwrap();
    assert_eq!((nexus.mod_id.as_str(), nexus.file_id.as_deref(), nexus.version.as_deref()), ("4242", Some("17001"), Some("2.1")));
    let hint = helmets.profile.as_ref().unwrap();
    assert_eq!((hint.enabled, hint.order), (true, 1), "the separator doesn't count");
    assert_eq!(hint.toggled, Some(vec![false, true]));
    assert_eq!(hint.selected, Some(vec![0, 1]));

    let report = run_import(&state, ids(&scan), &no_cancel(), |_| {}).await;
    assert_eq!(report.imported.len(), 2, "{:?}", report.failed);
    let h = report.imported.iter().find(|m| m.name == "Better Helmets").unwrap();
    assert_eq!(h.guid.to_string(), "0b8f3c1e-8a41-4b52-9d2c-3f1e2d4c5b6a");
    assert!(matches!(&h.config, Some(Config::V1 { toggled, selected, enabled: true, .. })
        if toggled == &vec![false, true] && selected == &vec![0, 1]));
    let local_mod = report.imported.iter().find(|m| m.name == "My Local Mod").unwrap();
    assert_eq!(local_mod.guid.to_string(), "5d6e7f80-1111-4222-8333-944455566677");
    assert_eq!((local_mod.enabled, local_mod.order), (Some(false), Some(0)));

    // The written manifest carries Arsenal's options, so deploy uses them.
    let mods = state.mods.lock().await.clone().unwrap();
    let m = mods.iter().find(|m| m.name() == "Better Helmets").unwrap();
    let written = Manifest::parse(&std::fs::read(m.directory.join("manifest.json")).unwrap(), "test").unwrap();
    match written {
        Manifest::V1(v) => {
            let opts = v.options.unwrap();
            assert_eq!(opts.len(), 2);
            assert_eq!(opts[1].sub_options.as_ref().unwrap()[1].include, vec![PathBuf::from("Shiny/Silver")]);
        }
        other => panic!("expected a v1 manifest, got {other:?}"),
    }
    let sidecar = sources::load_origin_sidecar(&m.directory).await.unwrap();
    assert_eq!(sidecar.installed_files[0].file_id.as_deref(), Some("17001"));
    assert_eq!(sidecar.installed_files[0].uploaded_at, Some(1719000000));
    // After a reload from disk, too.
    drop(mods);
    let mut fresh = None;
    let reloaded = crate::commands::mods::ensure_mods_loaded(&mut fresh, &base).await.unwrap();
    assert!(reloaded.iter().any(|m| m.guid().to_string() == "0b8f3c1e-8a41-4b52-9d2c-3f1e2d4c5b6a"));

    assert_eq!(snapshot(&data), before);
    let rescan = scan_folder(&data, &installed_index(&state).await, &no_cancel(), |_| {}).unwrap();
    assert!(rescan.items.iter().all(|i| matches!(i.status, ItemStatus::Installed { .. })), "{:?}", rescan.items);
}

/// Helldivers 2 Mod Manager 1.x: `enabled.json` (not profiles.json) next to
/// `Mods\`, in load order.
#[tokio::test]
async fn reads_the_1x_enabled_list() {
    let root = tempfile::tempdir().unwrap();
    let storage = hd2mm_storage(root.path());
    std::fs::remove_file(storage.join("profiles.json")).unwrap();
    std::fs::write(
        storage.join("enabled.json"),
        br#"[{"Guid":"aaaaaaaa-0000-0000-0000-000000000001","Enabled":true,"Toggled":[true,false],"Selected":[0,0]},
             {"Guid":"4c4f4341-4c01-0203-0405-060708090a0b","Enabled":false,"Toggled":[],"Selected":[0]}]"#,
    )
    .unwrap();
    let scan = scan_folder(&storage.join("Mods"), &InstalledIndex::default(), &no_cancel(), |_| {}).unwrap();
    let capes = scan.items.iter().find(|i| i.name == "Fancy Capes").unwrap().profile.clone().unwrap();
    assert_eq!((capes.enabled, capes.order, capes.toggled), (true, 0, Some(vec![true, false])));
    let loose = scan.items.iter().find(|i| i.name == "loose-mod").unwrap().profile.clone().unwrap();
    assert_eq!((loose.enabled, loose.order), (false, 1));
    assert!(scan.items.iter().find(|i| i.name == "Quiet Guns").unwrap().profile.is_none());

    // The 2024 predecessor: a map of the enabled mods' GUIDs to their
    // chosen option.
    std::fs::write(storage.join("enabled.json"), br#"{"aaaaaaaa-0000-0000-0000-000000000003": 0}"#).unwrap();
    let scan = scan_folder(&storage.join("Mods"), &InstalledIndex::default(), &no_cancel(), |_| {}).unwrap();
    let quiet = scan.items.iter().find(|i| i.name == "Quiet Guns").unwrap().profile.clone().unwrap();
    assert!(quiet.enabled);
}

#[test]
fn reads_the_staging_folders_patch_order() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("Vortex/helldivers2");
    let staging = game.join("mods");
    for name in ["A-1-1-0-1715000000", "B-2-1-0-1715000000"] {
        std::fs::create_dir_all(staging.join(name)).unwrap();
        std::fs::write(staging.join(name).join(PATCH), name.as_bytes()).unwrap();
    }
    std::fs::write(
        game.join("abc123_patch_order.json"),
        br#"[{"id":"x","modId":"B-2-1-0-1715000000","name":"B","enabled":true,"locked":false,"data":{"archives":[]}},
             {"id":"y","modId":"A-1-1-0-1715000000","name":"A","enabled":false,"locked":false,"data":{"archives":[]}}]"#,
    )
    .unwrap();
    let scan = scan_folder(&staging, &InstalledIndex::default(), &no_cancel(), |_| {}).unwrap();
    let a = scan.items.iter().find(|i| i.name == "A").unwrap().profile.clone().unwrap();
    let b = scan.items.iter().find(|i| i.name == "B").unwrap().profile.clone().unwrap();
    assert_eq!((a.enabled, a.order), (false, 1));
    assert_eq!((b.enabled, b.order), (true, 0));
}

#[test]
fn a_folder_of_mod_folders_is_not_one_mod() {
    let root = tempfile::tempdir().unwrap();
    let collection = root.path().join("my mods");
    for name in ["One", "Two"] {
        std::fs::create_dir_all(collection.join(name)).unwrap();
        std::fs::write(collection.join(name).join(PATCH), name.as_bytes()).unwrap();
        std::fs::write(
            collection.join(name).join("manifest.json"),
            format!(r#"{{"Guid":"{}","Name":"{name}","Description":"","IconPath":null,"Options":null}}"#, Uuid::new_v4()),
        )
        .unwrap();
    }
    let scan = scan_folder(root.path(), &InstalledIndex::default(), &no_cancel(), |_| {}).unwrap();
    let mut names: Vec<_> = scan.items.iter().map(|i| i.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["One", "Two"]);
}
