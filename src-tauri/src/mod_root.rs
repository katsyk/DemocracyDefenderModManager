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

/// A single plain folder or file name, safe to join onto a folder: exactly
/// one normal path component (no separators, not `.` or `..`, no root or
/// drive such as `C:`), and no `:` anywhere (a drive prefix or an NTFS
/// stream on Windows). Checked the same way on every platform, so a record
/// written on one can't escape the storage on another.
fn is_plain_name(name: &str) -> bool {
    use std::path::Component;
    if name.is_empty() || name.contains([':', '/', '\\', '\0']) {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!((components.next(), components.next()), (Some(Component::Normal(c)), None) if c == std::ffi::OsStr::new(name))
}

/// Whether `path` is a real folder: there, a folder, and not a symlink (or
/// a junction on Windows).
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// How far the wrapper folders `root` are still there inside `top`:
/// `Ok(true)` when the whole chain down to the mod's root is, `Ok(false)`
/// when it stops early (the rest was moved away or removed). An error when
/// `top` or any part of the chain that is there isn't a real folder (a
/// symlink, a junction, a file): then nothing inside it may be touched.
fn wrapper_chain(top: &Path, root: &[String]) -> std::io::Result<bool> {
    let unsafe_path = |p: &Path| std::io::Error::other(format!("{p:?} isn't a real folder (a link, or a file)"));
    if !is_real_dir(top) {
        return Err(unsafe_path(top));
    }
    for depth in 1..=root.len() {
        let level = top.join(join(&root[..depth]));
        match std::fs::symlink_metadata(&level) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
            Ok(_) if !is_real_dir(&level) => return Err(unsafe_path(&level)),
            Ok(_) => {}
        }
    }
    Ok(true)
}

/// What's next to the wrapper folders `root` (see
/// [`folder_root_components`]) in `top`: every item directly in `top` and
/// in each wrapper folder above the mod's root, except the next wrapper
/// folder down. Levels already gone are skipped. Symlinks are listed as
/// they are, never followed; a wrapper folder that is one is an error
/// (see [`wrapper_chain`]).
fn items_beside_wrappers(top: &Path, root: &[String]) -> std::io::Result<Vec<(PathBuf, String)>> {
    wrapper_chain(top, root)?;
    let mut out = Vec::new();
    for (depth, next) in root.iter().enumerate() {
        let level = top.join(join(&root[..depth]));
        if !is_real_dir(&level) {
            break;
        }
        for entry in std::fs::read_dir(&level)? {
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
    if !wrapper_chain(mod_dir, root)? {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{:?} isn't there", mod_dir.join(join(root)))));
    }
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

impl InPlaceRecord {
    /// Only plain names (see [`is_plain_name`]): the record is read back
    /// from disk, and everything it names is joined onto the storage.
    fn is_safe(&self) -> bool {
        is_plain_name(&self.folder) && !self.folder.starts_with('.') && !self.root.is_empty() && self.root.iter().all(|c| is_plain_name(c))
    }
}

/// The record file for the working folder `stage`.
fn in_place_record_path(stage: &Path) -> PathBuf {
    let mut name = stage.file_name().unwrap_or_default().to_os_string();
    name.push(crate::mod_folder::IN_PLACE_UNWRAP_RECORD_SUFFIX);
    stage.with_file_name(name)
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

/// Why [`unwrap_in_place`] failed, and so what the user needs to know.
#[derive(Debug)]
pub(crate) enum UnwrapFailure {
    /// Nothing was changed (the folder couldn't be set aside, or was put
    /// back right away).
    Unchanged(anyhow::Error),
    /// The folder is set aside as the hidden working folder `stage` and
    /// couldn't be put back yet; the next start does.
    SetAside { stage: PathBuf, error: anyhow::Error },
}

impl std::fmt::Display for UnwrapFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnwrapFailure::Unchanged(e) | UnwrapFailure::SetAside { error: e, .. } => write!(f, "{e:#}"),
        }
    }
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
    anyhow::ensure!(wrapper_chain(stage, root)?, "{:?} isn't there", stage.join(join(root)));
    let inner = stage.join(join(root));
    rename_with_retries(&inner, mod_dir).map_err(|e| anyhow::Error::from(e).context(format!("couldn't move {:?} to {:?}", inner, mod_dir)))
}

/// Step 3: move what was next to the wrapper folders into the mod's folder
/// `target` (an OS file whose name is taken there is deleted instead),
/// remove the then empty wrapper folders and the working folder, then the
/// record. Nothing else is ever deleted: anything that can't be moved
/// keeps the working folder (and its record), and the next start tries
/// again. A wrapper folder that turns out to be a link (or `target` that
/// isn't a real folder) stops it before anything is touched. Returns
/// whether it all went.
fn finish_in_place(stage: &Path, target: &Path, root: &[String]) -> bool {
    if !is_real_dir(target) {
        log::warn!("Not finishing {:?}: {:?} isn't a real folder.", stage, target);
        return false;
    }
    let items = match items_beside_wrappers(stage, root) {
        Ok(items) => items,
        Err(e) => {
            log::warn!("Not finishing {:?}: {}", stage, e);
            return false;
        }
    };
    for (path, name) in items {
        let dest = target.join(&name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            if is_os_file(&name) {
                // A link is removed itself, never what it points to.
                let removed = if is_real_dir(&path) { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
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
    // Innermost first; `remove_dir` only removes empty folders (and never
    // follows a link).
    for depth in (0..root.len()).rev() {
        let _ = std::fs::remove_dir(stage.join(join(&root[..depth])));
    }
    if std::fs::symlink_metadata(stage).is_ok() {
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
pub(crate) fn unwrap_in_place(mods_root: &Path, mod_dir: &Path, root: &[String]) -> Result<(), UnwrapFailure> {
    let unchanged = UnwrapFailure::Unchanged;
    crate::mod_folder::ensure_mod_folder(mods_root, mod_dir).map_err(unchanged)?;
    if root.is_empty() || !root.iter().all(|c| is_plain_name(c)) {
        return Err(unchanged(anyhow::anyhow!("not a wrapper folder path: {:?}", root)));
    }
    match wrapper_chain(mod_dir, root) {
        Ok(true) => {}
        Ok(false) => return Err(unchanged(anyhow::anyhow!("{:?} isn't there", mod_dir.join(join(root))))),
        Err(e) => return Err(unchanged(e.into())),
    }
    let stage = set_aside_for_unwrap(mods_root, mod_dir, root).map_err(unchanged)?;
    if let Err(e) = move_root_into_place(&stage, mod_dir, root) {
        return match rename_with_retries(&stage, mod_dir) {
            Ok(()) => {
                let _ = std::fs::remove_file(in_place_record_path(&stage));
                Err(unchanged(e))
            }
            Err(back) => {
                log::error!("Couldn't put {:?} back as {:?} yet (done on the next start): {}", stage, mod_dir, back);
                Err(UnwrapFailure::SetAside { stage, error: e })
            }
        };
    }
    if !finish_in_place(&stage, mod_dir, root) {
        log::warn!(
            "The mod in {:?} is set up, but {:?} still holds files that couldn't be moved in; the next start tries again.",
            mod_dir,
            stage
        );
    }
    Ok(())
}

/// At startup, before the mods are read: put each wrapped folder that
/// [`unwrap_in_place`] was working on when DDMM stopped in order, by its
/// record (only plain names, see [`InPlaceRecord::is_safe`], and only
/// folders directly in `mods_root`). Decided by whether the mod's root is
/// still inside the working folder:
///
/// - still there, the mod's folder not: stopped between the two renames,
///   so the working folder is renamed back, as the folder was before;
/// - moved out, the mod's folder there: the rest is finished;
/// - anything else (both there, or neither; an unreadable record; a link
///   where a wrapper folder should be): left as it is, and logged on
///   every start.
///
/// Nothing but OS files is ever deleted. Returns the working folders left
/// as they are.
pub(crate) fn recover_in_place_unwraps(mods_root: &Path) -> Vec<PathBuf> {
    use crate::mod_folder::{ensure_mod_folder, IN_PLACE_UNWRAP_PREFIX, IN_PLACE_UNWRAP_RECORD_SUFFIX};
    let mut left = Vec::new();
    let Ok(entries) = std::fs::read_dir(mods_root) else { return left };
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
        if std::fs::symlink_metadata(&stage).is_err() {
            // Never started, or finished.
            let _ = std::fs::remove_file(&record_path);
            continue;
        }
        let mut leave = |why: String| {
            log::error!(
                "Left the hidden folder {:?} as it is: {why}. It holds files from a mod folder DDMM was setting up \
                 where it is; move them back by hand if you need them.",
                stage
            );
            left.push(stage.clone());
        };
        let record = std::fs::read(&record_path).ok().and_then(|data| serde_json::from_slice::<InPlaceRecord>(&data).ok());
        let Some(record) = record.filter(InPlaceRecord::is_safe) else {
            leave(format!("its record {:?} can't be read", record_path));
            continue;
        };
        let target = mods_root.join(&record.folder);
        if let Err(e) = ensure_mod_folder(mods_root, &stage).and_then(|()| ensure_mod_folder(mods_root, &target)) {
            leave(format!("{e:#}"));
            continue;
        }
        let root_inside = match wrapper_chain(&stage, &record.root) {
            Ok(inside) => inside,
            Err(e) => {
                leave(e.to_string());
                continue;
            }
        };
        let target_there = std::fs::symlink_metadata(&target).is_ok();
        match (root_inside, target_there) {
            (true, false) => match rename_with_retries(&stage, &target) {
                Ok(()) => {
                    log::info!("Put {:?} back as it was, after setting it up as a mod didn't finish.", target);
                    let _ = std::fs::remove_file(&record_path);
                }
                Err(e) => leave(format!("couldn't rename it back to {:?} yet: {e}", target)),
            },
            (false, true) => {
                if finish_in_place(&stage, &target, &record.root) {
                    log::info!("Finished setting up {:?} where it is.", target);
                } else {
                    leave(format!("some of it couldn't be moved into {:?}", target));
                }
            }
            (true, true) => leave(format!("{:?} exists, but the mod's files are still in the hidden folder", target)),
            (false, false) => leave(format!("the mod's folder {:?} is gone (removed since?)", target)),
        }
    }
    left
}

#[cfg(test)]
mod tests;
