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
//! Loose files next to the wrapper folder that aren't patch files (a
//! readme, a preview image, a `.url` shortcut) don't stop this. When the
//! mod is copied in (from an archive, or a folder outside the mod storage)
//! they aren't installed: they describe the download, not the mod, and the
//! archive or folder they came from is left as it is. When the folder is
//! already in the mod storage and is set up where it is
//! ([`unwrap_in_place`]), they are the user's only copy, so they are moved
//! into the mod's folder instead.
//!
//! Patch files next to the wrapper folder are game data, and it's not
//! clear which is the mod, so the top stays the mod's root, as it always
//! was: nothing is dropped, and
//! [`wrapped_manifest_beside_patch_files`] lets the install say so.
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
/// files). Never part of a mod, so safe to drop.
pub(crate) fn is_os_file(name: &str) -> bool {
    name.starts_with("._")
        || ["__MACOSX", ".DS_Store", "Thumbs.db", "desktop.ini", ".Spotlight-V100", ".Trashes"]
            .iter()
            .any(|c| c.eq_ignore_ascii_case(name))
}

/// OS files ([`is_os_file`]) and DDMM's own working folders. They never
/// count as a second top-level item.
pub(crate) fn is_os_clutter(name: &str) -> bool {
    use crate::mod_folder::{IN_PLACE_UNWRAP_PREFIX, PENDING_DELETE_PREFIX, UNWRAP_PREFIX, UPDATE_STAGING_PREFIX};
    is_os_file(name)
        // DDMM's own working folders (`.update-backup-` included).
        || [UNWRAP_PREFIX, IN_PLACE_UNWRAP_PREFIX, UPDATE_STAGING_PREFIX, PENDING_DELETE_PREFIX]
            .iter()
            .any(|p| name.starts_with(p))
}

/// One item directly inside a folder: its name and whether it is a folder.
type Child = (String, bool);

/// The relative path of the mod's root under a top whose levels `list`
/// reads (the children of a relative path, or `None` if unreadable):
/// `Some("")` when the top itself has a manifest.json, `Some("A/B")` when
/// it is inside the wrapper folder(s) `A/B`, `None` when there is no
/// manifest.json to go by.
///
/// Patch files at a level mean that level is the mod, not a wrapper --
/// except at the top when `patch_files_on_top` is set, which only
/// [`wrapped_manifest_beside_patch_files`] uses to describe that layout.
fn find_root(mut list: impl FnMut(&[String]) -> Option<Vec<Child>>, patch_files_on_top: bool) -> Option<Vec<String>> {
    let mut prefix: Vec<String> = Vec::new();
    for depth in 0..=MAX_WRAPPER_DEPTH {
        let children = list(&prefix)?;
        if children.iter().any(|(name, is_dir)| !is_dir && name.eq_ignore_ascii_case(MANIFEST_FILE)) {
            return Some(prefix);
        }
        if depth == MAX_WRAPPER_DEPTH {
            break;
        }
        if !(depth == 0 && patch_files_on_top) && children.iter().any(|(name, is_dir)| !is_dir && is_patch_filename(name)) {
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
    let Some(root) = find_root(list, false) else { return Ok(None) };
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

/// Reads the levels of `folder` for [`find_root`]. Symlinks are never
/// followed (nor listed).
fn folder_lister(folder: &Path) -> impl FnMut(&[String]) -> Option<Vec<Child>> + '_ {
    move |prefix: &[String]| {
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
    }
}

/// Whether `dir` has a manifest.json DDMM can read.
fn has_readable_manifest(dir: &Path) -> bool {
    manifest_in(dir).is_some_and(|data| crate::models::manifest::Manifest::parse(&data, "manifest.json").is_ok())
}

/// The wrapper folders (`["A", "B"]` for `folder/A/B`) between `folder`
/// and the mod's root inside it, when `folder` has no manifest.json and
/// wraps one that can be read (see the module docs). `None` when `folder`
/// is the mod's root itself. Symlinks are never followed.
pub(crate) fn folder_root_components(folder: &Path) -> Option<Vec<String>> {
    let root = find_root(folder_lister(folder), false).filter(|root| !root.is_empty())?;
    let dir = folder.join(join(&root));
    // A broken one is ignored, as in an archive: the folder is added the
    // way it always was.
    if !has_readable_manifest(&dir) {
        log::warn!("Ignoring the manifest.json in {:?}: it can't be read.", dir);
        return None;
    }
    Some(root)
}

/// The folder inside `folder` that is the mod's root: `folder` itself,
/// unless it has no manifest.json and wraps one (see the module docs).
/// Symlinks are never followed.
pub(crate) fn folder_mod_root(folder: &Path) -> PathBuf {
    match folder_root_components(folder) {
        Some(root) => {
            let root = folder.join(join(&root));
            log::info!("{:?} has no manifest.json of its own; using the one in {:?}", folder, root);
            root
        }
        None => folder.to_path_buf(),
    }
}

/// For a mod folder whose top holds patch files next to a single (wrapper)
/// folder with a readable manifest.json: that folder's path (`A/B`). A
/// manifest.json at the top is not looked at: it's the one DDMM generated
/// for the mod (only mods with a generated one are asked about). Such a mod is installed with
/// the top as its root, as it always was (see the module docs), and the
/// install warns that the manifest.json wasn't used.
pub(crate) fn wrapped_manifest_beside_patch_files(folder: &Path) -> Option<String> {
    let top_has_patch_files = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .any(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_patch_filename(&e.file_name().to_string_lossy()));
    if !top_has_patch_files {
        return None;
    }
    // The top's own manifest.json is DDMM's generated one by now.
    let mut list = folder_lister(folder);
    let without_top_manifest = |prefix: &[String]| {
        let mut children = list(prefix)?;
        if prefix.is_empty() {
            children.retain(|(name, is_dir)| *is_dir || !name.eq_ignore_ascii_case(MANIFEST_FILE));
        }
        Some(children)
    };
    let root = find_root(without_top_manifest, true).filter(|root| !root.is_empty())?;
    has_readable_manifest(&folder.join(join(&root))).then(|| root.join("/"))
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

// --- A wrapped folder set up where it is, in the mod storage ---

/// What's next to the wrapper folders `root` (see
/// [`folder_root_components`]) in `top`: every item directly in `top` and
/// in each wrapper folder above the mod's root, except the next wrapper
/// folder down. Symlinks are listed as they are, never followed.
fn items_beside_wrappers(top: &Path, root: &[String]) -> std::io::Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    for (depth, next) in root.iter().enumerate() {
        for entry in std::fs::read_dir(top.join(join(&root[..depth])))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if &name != next {
                out.push((entry.path(), name));
            }
        }
    }
    Ok(out)
}

/// Before [`unwrap_in_place`]: the name of an item next to the wrapper
/// folders of `mod_dir` that can't be moved into the mod's root `root`
/// because the mod's root (or another such item) already has one by that
/// name, ignoring letter case. OS files ([`is_os_file`]) never clash:
/// they are dropped instead. `None` when everything fits. Only reads.
pub(crate) fn in_place_clash(mod_dir: &Path, root: &[String]) -> std::io::Result<Option<String>> {
    let mut taken = Vec::new();
    for entry in std::fs::read_dir(mod_dir.join(join(root)))? {
        taken.push(entry?.file_name().to_string_lossy().to_lowercase());
    }
    for (_, name) in items_beside_wrappers(mod_dir, root)? {
        let key = name.to_lowercase();
        if taken.contains(&key) {
            if is_os_file(&name) {
                continue;
            }
            return Ok(Some(name));
        }
        taken.push(key);
    }
    Ok(None)
}

/// What [`unwrap_in_place`] writes next to its working folder before
/// touching anything, so the next start can finish or undo it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct InPlaceRecord {
    /// The mod's folder in the storage.
    folder: String,
    /// The wrapper folders down to the mod's root.
    root: Vec<String>,
}

/// The record file for the working folder `stage`.
fn in_place_record_path(stage: &Path) -> PathBuf {
    let mut name = stage.file_name().unwrap_or_default().to_os_string();
    name.push(crate::mod_folder::IN_PLACE_UNWRAP_RECORD_SUFFIX);
    stage.with_file_name(name)
}

/// A single plain name: no separators, not `.` or `..`.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// `std::fs::rename`, retried a few times a moment apart (on Windows a
/// virus scanner or the search indexer often has a new file open briefly).
fn rename_with_retries(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = std::fs::rename(from, to);
    for delay in crate::mod_folder::REMOVE_RETRY_DELAYS {
        if attempt.is_ok() {
            break;
        }
        std::thread::sleep(*delay);
        attempt = std::fs::rename(from, to);
    }
    attempt
}

/// Step 1 of [`unwrap_in_place`]: record what is about to happen, then
/// rename the whole of `mod_dir` to a working folder in the storage (one
/// rename: either it worked, or nothing changed). Returns the working
/// folder.
fn set_aside_for_unwrap(mods_root: &Path, mod_dir: &Path, root: &[String]) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let folder = mod_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow::anyhow!("{:?} has no name", mod_dir))?;
    let stage = mods_root.join(format!("{}{}", crate::mod_folder::IN_PLACE_UNWRAP_PREFIX, uuid::Uuid::new_v4()));
    crate::mod_folder::ensure_mod_folder(mods_root, &stage)?;
    let record = in_place_record_path(&stage);
    let data = serde_json::to_vec(&InPlaceRecord { folder, root: root.to_vec() })?;
    std::fs::write(&record, data).with_context(|| format!("couldn't write {:?}", record))?;
    if let Err(e) = rename_with_retries(mod_dir, &stage) {
        let _ = std::fs::remove_file(&record);
        return Err(anyhow::Error::from(e).context(format!("couldn't move {:?} to {:?}", mod_dir, stage)));
    }
    Ok(stage)
}

/// Step 2: the mod's root takes the mod folder's place (one rename).
fn move_root_into_place(stage: &Path, mod_dir: &Path, root: &[String]) -> anyhow::Result<()> {
    let inner = stage.join(join(root));
    rename_with_retries(&inner, mod_dir).map_err(|e| anyhow::Error::from(e).context(format!("couldn't move {:?} to {:?}", inner, mod_dir)))
}

/// Step 3: move what was next to the wrapper folders into the mod's folder
/// `target` (an OS file whose name is taken there is deleted instead),
/// remove the then empty wrapper folders and the working folder, then the
/// record. Nothing else is ever deleted: anything that can't be moved
/// keeps the working folder (and its record), and the next start tries
/// again. Returns whether it all went.
fn finish_in_place(stage: &Path, target: &Path, root: &[String]) -> bool {
    match items_beside_wrappers(stage, root) {
        Ok(items) => {
            for (path, name) in items {
                let dest = target.join(&name);
                if std::fs::symlink_metadata(&dest).is_ok() {
                    if is_os_file(&name) {
                        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
                        let removed = if is_dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                        if let Err(e) = removed {
                            log::warn!("Couldn't remove {:?}: {}", path, e);
                        }
                    } else {
                        log::warn!("Not moving {:?} into {:?}: something there already has its name.", path, target);
                    }
                    continue;
                }
                if let Err(e) = rename_with_retries(&path, &dest) {
                    log::warn!("Couldn't move {:?} to {:?}: {}", path, dest, e);
                }
            }
        }
        Err(e) => log::warn!("Couldn't read {:?}: {}", stage, e),
    }
    // Innermost first; `remove_dir` only removes empty folders.
    for depth in (0..root.len()).rev() {
        let _ = std::fs::remove_dir(stage.join(join(&root[..depth])));
    }
    if std::fs::symlink_metadata(stage).is_ok() {
        log::warn!("Left {:?} as it is (not empty); the next start tries again.", stage);
        return false;
    }
    let _ = std::fs::remove_file(in_place_record_path(stage));
    true
}

/// Set up a wrapped mod folder in the mod storage where it is: the
/// folder `mod_dir` (directly in `mods_root`) has no manifest.json, and
/// its mod's root is inside the wrapper folders `root` (see
/// [`folder_root_components`]). Afterwards `mod_dir` holds the mod's root
/// contents, as an install from the zipped folder would, plus whatever was
/// next to the wrapper folders (see [`in_place_clash`], which the caller
/// checks first). The mod keeps its folder, so `mod_dir` stays the single
/// place it lives in, for deploy, update, replace and delete alike.
///
/// Crash safe: a record is written first, then `mod_dir` is renamed to a
/// `.adopt-<id>` working folder and the mod's root renamed to `mod_dir`,
/// each in one step. [`recover_in_place_unwraps`] puts a folder back if
/// DDMM stopped between the two, and finishes the rest otherwise.
pub(crate) fn unwrap_in_place(mods_root: &Path, mod_dir: &Path, root: &[String]) -> anyhow::Result<()> {
    crate::mod_folder::ensure_mod_folder(mods_root, mod_dir)?;
    anyhow::ensure!(!root.is_empty() && root.iter().all(|c| is_plain_name(c)), "not a wrapper folder path: {:?}", root);
    let stage = set_aside_for_unwrap(mods_root, mod_dir, root)?;
    if let Err(e) = move_root_into_place(&stage, mod_dir, root) {
        match rename_with_retries(&stage, mod_dir) {
            Ok(()) => {
                let _ = std::fs::remove_file(in_place_record_path(&stage));
            }
            Err(back) => log::error!("Couldn't put {:?} back as {:?} yet (done on the next start): {}", stage, mod_dir, back),
        }
        return Err(e);
    }
    finish_in_place(&stage, mod_dir, root);
    Ok(())
}

/// At startup, before the mods are read: put each wrapped folder that
/// [`unwrap_in_place`] was working on when DDMM stopped in order, by its
/// record. If the mod's folder isn't there (stopped between the two
/// renames), the working folder is renamed back to it, as it was before;
/// if it is, the rest is finished. A working folder without a readable
/// record, or whose mod root is still inside it while a folder of the
/// mod's name exists, is left alone. Nothing but OS files is deleted.
pub(crate) fn recover_in_place_unwraps(mods_root: &Path) {
    use crate::mod_folder::{IN_PLACE_UNWRAP_PREFIX, IN_PLACE_UNWRAP_RECORD_SUFFIX};
    let Ok(entries) = std::fs::read_dir(mods_root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_prefix(IN_PLACE_UNWRAP_PREFIX).and_then(|r| r.strip_suffix(IN_PLACE_UNWRAP_RECORD_SUFFIX)) else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_err() || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let record_path = entry.path();
        let stage = mods_root.join(format!("{IN_PLACE_UNWRAP_PREFIX}{id}"));
        let record = std::fs::read(&record_path)
            .ok()
            .and_then(|data| serde_json::from_slice::<InPlaceRecord>(&data).ok())
            .filter(|r| {
                is_plain_name(&r.folder) && !r.folder.starts_with('.') && !r.root.is_empty() && r.root.iter().all(|c| is_plain_name(c))
            });
        let Some(record) = record else {
            log::warn!("Leaving {:?} and {:?} as they are: the record can't be read.", stage, record_path);
            continue;
        };
        let target = mods_root.join(&record.folder);
        if std::fs::symlink_metadata(&stage).is_err() {
            // Never started, or finished.
            let _ = std::fs::remove_file(&record_path);
            continue;
        }
        if std::fs::symlink_metadata(&target).is_err() {
            match rename_with_retries(&stage, &target) {
                Ok(()) => {
                    log::info!("Put {:?} back as it was, after setting it up as a mod didn't finish.", target);
                    let _ = std::fs::remove_file(&record_path);
                }
                Err(e) => log::warn!("Couldn't put {:?} back as {:?} yet: {}", stage, target, e),
            }
            continue;
        }
        if std::fs::symlink_metadata(stage.join(join(&record.root))).is_ok() {
            log::warn!("Leaving {:?} as it is: {:?} exists, but the mod's files are still in the working folder.", stage, target);
            continue;
        }
        if finish_in_place(&stage, &target, &record.root) {
            log::info!("Finished setting up {:?} where it is.", target);
        }
    }
}

#[cfg(test)]
mod tests;
