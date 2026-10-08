//! Where a mod's root is inside an archive or a folder.
//!
//! Normally that is the archive or folder itself. But many authors zip the
//! mod's folder instead of its contents, so the archive holds
//! `ModName/manifest.json` rather than `manifest.json`. When the top has no
//! manifest.json and -- ignoring OS clutter such as `__MACOSX` or
//! `.DS_Store` -- exactly one folder and no patch files, that folder is
//! looked into, up to [`MAX_WRAPPER_DEPTH`] levels down. If a manifest.json
//! turns up there, that folder is the mod's root: its manifest is read,
//! and only its contents are installed, so the manifest's paths (`Include`,
//! `Options`, images) resolve exactly as the author wrote them.
//!
//! A wrapper without a manifest.json anywhere is left alone: those mods get
//! a generated manifest, and patch-layout detection
//! ([`crate::utils::detect_patch_layout`]) already finds patch files inside
//! wrapper folders.

use std::path::{Path, PathBuf};

use crate::{archive::Archive, commands::mods::MANIFEST_FILE, utils::is_patch_filename};

/// How many nested wrapper folders are looked through.
pub(crate) const MAX_WRAPPER_DEPTH: usize = 3;

/// Files and folders operating systems and archivers drop next to a mod
/// (`__MACOSX` holds a Mac's resource forks, `._name` its AppleDouble
/// files), and DDMM's own working folders. They never count as a second
/// top-level item.
pub(crate) fn is_os_clutter(name: &str) -> bool {
    use crate::mod_folder::{PENDING_DELETE_PREFIX, UNWRAP_PREFIX, UPDATE_STAGING_PREFIX};
    name.starts_with("._")
        // DDMM's own working folders (`.update-backup-` included).
        || [UNWRAP_PREFIX, UPDATE_STAGING_PREFIX, PENDING_DELETE_PREFIX].iter().any(|p| name.starts_with(p))
        || ["__MACOSX", ".DS_Store", "Thumbs.db", "desktop.ini", ".Spotlight-V100", ".Trashes"]
            .iter()
            .any(|c| c.eq_ignore_ascii_case(name))
}

/// One item directly inside a folder: its name and whether it is a folder.
type Child = (String, bool);

/// The relative path of the mod's root under a top whose levels `list`
/// reads (the children of a relative path, or `None` if unreadable):
/// `Some("")` when the top itself has a manifest.json, `Some("A/B")` when
/// it is inside the wrapper folder(s) `A/B`, `None` when there is no
/// manifest.json to go by.
fn find_root(mut list: impl FnMut(&[String]) -> Option<Vec<Child>>) -> Option<Vec<String>> {
    let mut prefix: Vec<String> = Vec::new();
    for depth in 0..=MAX_WRAPPER_DEPTH {
        let children = list(&prefix)?;
        if children.iter().any(|(name, is_dir)| !is_dir && name.eq_ignore_ascii_case(MANIFEST_FILE)) {
            return Some(prefix);
        }
        if depth == MAX_WRAPPER_DEPTH {
            break;
        }
        // Patch files here mean this level is the mod, not a wrapper.
        if children.iter().any(|(name, is_dir)| !is_dir && is_patch_filename(name)) {
            return None;
        }
        let mut folders = children.iter().filter(|(name, is_dir)| *is_dir && !is_os_clutter(name));
        let (Some((only, _)), None) = (folders.next(), folders.next()) else {
            return None;
        };
        prefix.push(only.clone());
    }
    None
}

fn join(components: &[String]) -> PathBuf {
    components.iter().collect()
}

/// An archive entry's path as components, the way extraction lays it out
/// (`\` is a separator, `.` and empty components are dropped).
fn components(raw: &Path) -> Vec<String> {
    raw.to_string_lossy()
        .replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_string)
        .collect()
}

/// Find the mod's manifest.json in `archive`, at its root or inside
/// wrapper folders (see the module docs). Returns the mod root's path
/// inside the archive (empty for the archive root) and the manifest's raw
/// entry path, ready for [`Archive::read_path`]. `manifest.json` wins over
/// a case variant such as `Manifest.json`.
pub(crate) fn find_archive_manifest(archive: &mut Archive) -> anyhow::Result<Option<(PathBuf, PathBuf)>> {
    let mut entries = Vec::new();
    for entry in archive.iter()? {
        let entry = entry?;
        entries.push((components(entry.path()), entry.is_directory(), entry.path().to_path_buf()));
    }
    let list = |prefix: &[String]| {
        let mut children: Vec<Child> = Vec::new();
        for (comps, is_dir, _) in &entries {
            if comps.len() > prefix.len() && comps.starts_with(prefix) {
                let name = &comps[prefix.len()];
                let child_is_dir = *is_dir || comps.len() > prefix.len() + 1;
                if !children.iter().any(|(n, d)| n == name && *d == child_is_dir) {
                    children.push((name.clone(), child_is_dir));
                }
            }
        }
        Some(children)
    };
    let Some(root) = find_root(list) else { return Ok(None) };
    let manifest_at = |exact: bool| {
        entries.iter().find(|(comps, is_dir, _)| {
            !is_dir
                && comps.len() == root.len() + 1
                && comps.starts_with(&root)
                && if exact { comps[root.len()] == MANIFEST_FILE } else { comps[root.len()].eq_ignore_ascii_case(MANIFEST_FILE) }
        })
    };
    let entry = manifest_at(true).or_else(|| manifest_at(false)).map(|(_, _, raw)| raw.clone());
    Ok(entry.map(|raw| (join(&root), raw)))
}

/// The folder inside `folder` that is the mod's root: `folder` itself,
/// unless it has no manifest.json and wraps one (see the module docs).
/// Symlinks are never followed.
pub(crate) fn folder_mod_root(folder: &Path) -> PathBuf {
    let list = |prefix: &[String]| {
        let entries = std::fs::read_dir(folder.join(join(prefix))).ok()?;
        let mut children = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            children.push((entry.file_name().to_string_lossy().into_owned(), file_type.is_dir()));
        }
        Some(children)
    };
    match find_root(list) {
        Some(root) if !root.is_empty() => {
            let root = folder.join(join(&root));
            // A broken one is ignored, as in an archive: the folder is
            // added the way it always was.
            if manifest_in(&root).is_none_or(|data| crate::models::manifest::Manifest::parse(&data, "manifest.json").is_err()) {
                log::warn!("Ignoring the manifest.json in {:?}: it can't be read.", root);
                return folder.to_path_buf();
            }
            log::info!("{:?} has no manifest.json of its own; using the one in {:?}", folder, root);
            root
        }
        _ => folder.to_path_buf(),
    }
}

/// The contents of `dir`'s manifest.json (or a case variant of its name).
fn manifest_in(dir: &Path) -> Option<Vec<u8>> {
    let exact = dir.join(MANIFEST_FILE);
    if exact.is_file() {
        return std::fs::read(exact).ok();
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let is_file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        (is_file && e.file_name().to_string_lossy().eq_ignore_ascii_case(MANIFEST_FILE)).then(|| std::fs::read(e.path()).ok()).flatten()
    })
}

#[cfg(test)]
mod tests {
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
}
