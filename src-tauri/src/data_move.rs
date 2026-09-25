//! Moving DDMM's data folder: "Change" and "Reset to default" next to the
//! data folder in Settings. See `docs/using/data-folder.md`.
//!
//! Safety rules, in order:
//!
//! 1. [`plan`] refuses anything that could nest a folder inside itself (the
//!    destination is, is inside, or contains the current data folder), touch
//!    the game install, or mix DDMM's files into an unrelated folder.
//! 2. [`execute`] copies into a temporary folder *inside* the destination,
//!    checks every file's size against what was copied, then renames the
//!    items into place. Only after that does it call `commit` (which writes
//!    the pointer file). Any failure before or during `commit` removes
//!    everything it created and leaves the old data and pointer untouched.
//! 3. [`cleanup_old`] deletes the old copy only after the commit. Anything it
//!    can't delete (a log file still open on Windows) is recorded and retried
//!    at the next start by [`run_pending_cleanup`].
//!
//! Only DDMM's own items ([`MOVED_ITEMS`]) are moved. The data folder can be
//! shared with other files: the executable itself in a portable copy, or the
//! webview's own cache in the Linux app data folder.

use std::{
    collections::BTreeMap,
    io,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context};
use serde::Serialize;

use crate::fs_util::{is_cross_device, path_overlap, PathOverlap};

/// DDMM's own files and folders in the data folder, copied by a move.
pub const MOVED_ITEMS: &[&str] = &[
    "mods",
    "settings.json",
    "profiles.json",
    "update-cache.json",
    "nexus-api-key",
    "nexus-oauth-tokens",
    "logs",
];

/// DDMM's own items that are recreated as needed (download staging, the
/// Windows native messaging manifests, which are rewritten at every start,
/// and `bridge.json`, which the restarted app writes again in the new
/// folder): never copied, but removed from the old folder after a move.
pub const REGENERATED_ITEMS: &[&str] = &[".downloads", "native-messaging", "bridge.json"];

/// Items that may already exist in the default folder when moving back
/// there ("Reset to default") without making it an existing DDMM data
/// folder: leftovers DDMM itself recreates.
const STALE_ITEMS: &[&str] = &["logs"];

/// Files some operating systems drop into any folder they display. A folder
/// with only these in it counts as empty.
const OS_CLUTTER: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];

/// Created in the destination before copying and removed only once
/// everything is in place, so a folder left behind by a crash (or power
/// loss) mid-move is never mistaken for complete data.
pub const INCOMPLETE_MARKER: &str = ".ddmm-move-incomplete";
const TEMP_PREFIX: &str = ".ddmm-move-";
/// Holds a stale `logs` folder of the default location while moving back.
const STALE_ASIDE: &str = "stale";
/// Old items [`cleanup_old`] couldn't delete, retried at the next start.
pub const CLEANUP_FILE: &str = ".ddmm-cleanup-pending.json";
/// Created inside a non-empty folder the user picked, so DDMM's files are
/// never mixed in with someone else's.
pub const SUBFOLDER_NAME: &str = "DDMM Data";
/// Headroom kept free on the destination beyond the data itself.
const FREE_SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// What a move would do, shown in the confirmation before anything happens.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct MovePlan {
    /// The current data folder.
    pub source: PathBuf,
    /// The folder the user picked.
    pub picked: PathBuf,
    /// Where the data will actually live (`picked`, or `picked/DDMM Data`).
    pub target: PathBuf,
    /// Moving back to the default location (the pointer file is removed).
    pub is_reset: bool,
    /// `target` is a `DDMM Data` folder inside a non-empty `picked`.
    pub used_subfolder: bool,
    /// `target` already holds DDMM data: it can be used as is ("adopt"),
    /// but DDMM won't copy over it.
    pub existing_data: bool,
    pub total_bytes: u64,
    pub total_files: u64,
    /// Free space on the destination drive, if it could be determined.
    pub free_bytes: Option<u64>,
}

pub struct PlanInput<'a> {
    pub current: &'a Path,
    pub picked: &'a Path,
    pub default_path: &'a Path,
    /// The game install folder from settings, if one is set.
    pub game_path: Option<&'a Path>,
    pub reset: bool,
}

/// Validate a move and work out where the data would go. Never changes
/// anything on disk.
pub fn plan(input: &PlanInput, free_space: &dyn Fn(&Path) -> Option<u64>) -> anyhow::Result<MovePlan> {
    let PlanInput { current, picked, default_path, game_path, reset } = *input;

    if !picked.is_absolute() {
        bail!("Pick a full folder path (\"{}\" isn't one).", picked.display());
    }
    let is_reset = reset || matches!(path_overlap(picked, default_path)?, Some(PathOverlap::Same));

    if is_reset {
        if matches!(path_overlap(current, default_path)?, Some(PathOverlap::Same)) {
            bail!("DDMM's data is already in the default location ({}).", default_path.display());
        }
    } else if !picked.is_dir() {
        bail!("{} doesn't exist or isn't a folder.", picked.display());
    }

    let picked = if is_reset { default_path } else { picked };
    check_not_overlapping_current(current, picked)?;

    // Where the data goes.
    let mut used_subfolder = false;
    let target = if is_reset || is_effectively_empty(picked)? || is_ddmm_data_dir(picked) {
        picked.to_path_buf()
    } else {
        used_subfolder = true;
        picked.join(SUBFOLDER_NAME)
    };
    check_not_overlapping_current(current, &target)?;

    if target.join(INCOMPLETE_MARKER).exists() {
        bail!(
            "{} contains an unfinished data folder move from an earlier attempt. Delete that \
             folder (after checking it doesn't hold anything you need) and try again.",
            target.display()
        );
    }

    let existing_data = target.is_dir() && is_ddmm_data_dir(&target);
    if !existing_data && target.exists() {
        if !target.is_dir() {
            bail!("{} exists but isn't a folder.", target.display());
        }
        if is_reset {
            let clashes: Vec<&str> = MOVED_ITEMS
                .iter()
                .copied()
                .filter(|name| !STALE_ITEMS.contains(name) && target.join(name).symlink_metadata().is_ok())
                .collect();
            if !clashes.is_empty() {
                bail!(
                    "The default location ({}) already has {} in it, but it isn't a complete DDMM \
                     data folder. Move or delete those first.",
                    target.display(),
                    clashes.join(", ")
                );
            }
        } else if used_subfolder && !is_effectively_empty(&target)? {
            bail!(
                "{} already has a \"{}\" folder with other files in it. Pick another folder.",
                picked.display(),
                SUBFOLDER_NAME
            );
        }
    }

    if let Some(game) = game_path.filter(|g| !g.as_os_str().is_empty()) {
        for p in [picked, target.as_path()] {
            if path_overlap(p, game)?.is_some() {
                bail!(
                    "{} is inside, or contains, the Helldivers 2 install ({}). Keep DDMM's data \
                     outside the game folder.",
                    p.display(),
                    game.display()
                );
            }
        }
    }

    let existing_ancestor = nearest_existing_ancestor(&target)
        .with_context(|| format!("{} isn't on any drive DDMM can see", target.display()))?;
    if !crate::data_dir::is_dir_writable(&existing_ancestor) {
        bail!("DDMM can't write to {}. Pick a folder you have write access to.", existing_ancestor.display());
    }

    let free_bytes = free_space(&existing_ancestor);
    let (total_bytes, total_files) = if existing_data {
        (0, 0)
    } else {
        let entries = scan_items(current)?;
        (entries.iter().map(|e| e.size).sum(), entries.iter().filter(|e| !e.is_dir).count() as u64)
    };

    if !existing_data {
        if let Some(free) = free_bytes {
            if free < total_bytes.saturating_add(FREE_SPACE_MARGIN) {
                bail!(
                    "Not enough free space on the destination drive: DDMM's data needs {} (plus {} \
                     headroom) and only {} is free.",
                    human_bytes(total_bytes),
                    human_bytes(FREE_SPACE_MARGIN),
                    human_bytes(free)
                );
            }
        }
    }

    Ok(MovePlan {
        source: current.to_path_buf(),
        picked: picked.to_path_buf(),
        target,
        is_reset,
        used_subfolder,
        existing_data,
        total_bytes,
        total_files,
        free_bytes,
    })
}

fn check_not_overlapping_current(current: &Path, dest: &Path) -> anyhow::Result<()> {
    match path_overlap(current, dest)? {
        None => Ok(()),
        Some(PathOverlap::Same) => bail!("{} is already DDMM's data folder.", dest.display()),
        Some(PathOverlap::SecondInsideFirst) => bail!(
            "{} is inside DDMM's current data folder ({}). Pick a folder outside it: copying a \
             folder into itself would never finish.",
            dest.display(),
            current.display()
        ),
        Some(PathOverlap::FirstInsideSecond) => bail!(
            "{} contains DDMM's current data folder ({}). Pick a folder that isn't above it.",
            dest.display(),
            current.display()
        ),
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|a| a.is_dir()).map(Path::to_path_buf)
}

/// Whether `dir` holds DDMM data: a `settings.json` or `profiles.json` that
/// parses as DDMM's own.
pub fn is_ddmm_data_dir(dir: &Path) -> bool {
    let parses = |name: &str, check: &dyn Fn(&[u8]) -> bool| {
        std::fs::read(dir.join(name)).map(|data| check(&data)).unwrap_or(false)
    };
    parses("settings.json", &|d| serde_json::from_slice::<crate::models::settings::Settings>(d).is_ok())
        || parses("profiles.json", &|d| serde_json::from_slice::<crate::models::profile::ProfilesConfig>(d).is_ok())
}

/// Empty, or holding only files the OS creates by itself.
fn is_effectively_empty(dir: &Path) -> anyhow::Result<bool> {
    if !dir.exists() {
        return Ok(true);
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("couldn't read {}", dir.display()))? {
        let name = entry?.file_name();
        if !OS_CLUTTER.iter().any(|c| name.to_str() == Some(*c)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One file or folder to copy, relative to the data folder.
#[derive(Debug, Clone)]
struct Entry {
    rel: PathBuf,
    is_dir: bool,
    size: u64,
}

/// Every file and folder under the [`MOVED_ITEMS`] that exist in `base`,
/// parents before children. Refuses symlinks (and Windows junctions)
/// anywhere: following one could copy far more than DDMM's data, and
/// skipping one would silently leave something behind.
fn scan_items(base: &Path) -> anyhow::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for name in MOVED_ITEMS {
        let path = base.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        scan_one(&path, PathBuf::from(name), meta, &mut entries)?;
    }
    Ok(entries)
}

fn scan_one(path: &Path, rel: PathBuf, meta: std::fs::Metadata, out: &mut Vec<Entry>) -> anyhow::Result<()> {
    if meta.file_type().is_symlink() {
        bail!(
            "{} is a symbolic link (or junction). DDMM won't move links; replace it with the real \
             folder or file, or move the data by hand.",
            path.display()
        );
    }
    if meta.is_dir() {
        out.push(Entry { rel: rel.clone(), is_dir: true, size: 0 });
        let mut children: Vec<_> = std::fs::read_dir(path)
            .with_context(|| format!("couldn't read {}", path.display()))?
            .collect::<Result<_, _>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let meta = std::fs::symlink_metadata(child.path())?;
            scan_one(&child.path(), rel.join(child.file_name()), meta, out)?;
        }
    } else if meta.is_file() {
        out.push(Entry { rel, is_dir: false, size: meta.len() });
    } else {
        bail!("{} isn't a regular file or folder; DDMM can't move it.", path.display());
    }
    Ok(())
}

/// Progress of [`execute`], sent to the UI.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct MoveProgress {
    /// `"copying"`, `"verifying"` or `"finishing"`.
    pub phase: &'static str,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub total_files: u64,
}

/// Copies one file: `(from, to, progress)`, see [`MoveOps::copy_file`].
pub type CopyFileFn<'a> = dyn Fn(&Path, &Path, &mut dyn FnMut(u64)) -> io::Result<u64> + 'a;

/// The file operations [`execute`] uses, injectable so tests can make any
/// step fail.
pub struct MoveOps<'a> {
    /// Copy one file, calling the callback with the bytes copied so far
    /// (a single mod file can be several GB); returns the bytes copied.
    pub copy_file: &'a CopyFileFn<'a>,
    pub rename: &'a dyn Fn(&Path, &Path) -> io::Result<()>,
}

/// Copy in 4 MB chunks with progress, keeping the source's permissions
/// (the optional Nexus key/sign-in files are owner-only, and are created
/// that way rather than chmod-ed afterwards), and flush the copy to disk
/// before it can be switched to.
fn real_copy(from: &Path, to: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<u64> {
    use std::io::{Read, Write};
    let mut src = std::fs::File::open(from)?;
    let perms = src.metadata()?.permissions();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(perms.mode() & 0o7777);
    }
    let mut dst = options.open(to)?;
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        total += n as u64;
        progress(total);
    }
    dst.sync_all()?;
    drop(dst);
    #[cfg(not(unix))]
    std::fs::set_permissions(to, perms)?;
    Ok(total)
}

fn real_rename(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub const REAL_OPS: MoveOps<'static> = MoveOps { copy_file: &real_copy, rename: &real_rename };

/// Copy DDMM's data to `plan.target`, verify it, put it in place and call
/// `commit` (which writes the pointer file). On any error, including from
/// `commit`, everything this created is removed again and the old data is
/// left exactly as it was. The old data is *not* deleted here: call
/// [`cleanup_old`] afterwards.
pub fn execute(
    plan: &MovePlan,
    ops: &MoveOps,
    commit: &mut dyn FnMut(&Path) -> anyhow::Result<()>,
    progress: &mut dyn FnMut(&MoveProgress),
) -> anyhow::Result<()> {
    if plan.existing_data {
        bail!("{} already holds DDMM data; use it as is instead of moving.", plan.target.display());
    }
    let target = &plan.target;
    let created_target = !target.exists();
    std::fs::create_dir_all(target).with_context(|| format!("couldn't create {}", target.display()))?;

    let marker = target.join(INCOMPLETE_MARKER);
    let tmp = target.join(format!("{TEMP_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    let mut placed: Vec<PathBuf> = Vec::new();
    let mut stale_aside: Option<(PathBuf, PathBuf)> = None;

    let result = (|| -> anyhow::Result<()> {
        std::fs::write(&marker, plan.source.to_string_lossy().as_bytes())
            .with_context(|| format!("couldn't write to {}", target.display()))?;
        std::fs::create_dir(&tmp).with_context(|| format!("couldn't create {}", tmp.display()))?;

        let entries = scan_items(&plan.source)?;
        let copied = copy_entries(&plan.source, &tmp, &entries, ops, progress)?;
        verify(&tmp, &entries, &copied, progress)?;

        progress(&MoveProgress { phase: "finishing", ..done_progress(&entries) });
        let mut items: Vec<_> = std::fs::read_dir(&tmp)?.collect::<Result<_, _>>()?;
        items.sort_by_key(|e| e.file_name());
        for item in items {
            let dest = target.join(item.file_name());
            if dest.symlink_metadata().is_ok() {
                let stale = plan.is_reset && STALE_ITEMS.iter().any(|s| item.file_name().to_str() == Some(*s));
                if !stale {
                    bail!("{} appeared while moving; nothing was changed.", dest.display());
                }
                // A stale `logs` in the default folder: set it aside inside
                // the temp folder (same drive, so a plain rename) and put it
                // back if anything fails.
                let aside = tmp.join(STALE_ASIDE);
                std::fs::rename(&dest, &aside).with_context(|| format!("couldn't move {} aside", dest.display()))?;
                stale_aside = Some((aside, dest.clone()));
            }
            move_into_place(&item.path(), &dest, ops)?;
            placed.push(dest);
        }

        rewrite_stored_paths(&plan.source, target)?;
        std::fs::remove_file(&marker).with_context(|| format!("couldn't remove {}", marker.display()))?;
        commit(target)
    })();

    match result {
        Ok(()) => {
            if let Err(e) = std::fs::remove_dir_all(&tmp) {
                log::warn!("Couldn't remove the temporary folder {tmp:?}: {e}");
            }
            Ok(())
        }
        Err(e) => {
            for p in placed.iter().rev() {
                let _ = remove_any(p);
            }
            if let Some((aside, original)) = stale_aside {
                let _ = std::fs::rename(aside, original);
            }
            let _ = std::fs::remove_dir_all(&tmp);
            let _ = std::fs::remove_file(&marker);
            if created_target {
                // Only removes it if it's empty again, as it should be.
                let _ = std::fs::remove_dir(target);
            }
            Err(e)
        }
    }
}

fn done_progress(entries: &[Entry]) -> MoveProgress {
    let total_bytes = entries.iter().map(|e| e.size).sum();
    let total_files = entries.iter().filter(|e| !e.is_dir).count() as u64;
    MoveProgress { phase: "copying", done_bytes: total_bytes, total_bytes, done_files: total_files, total_files }
}

fn copy_entries(
    source: &Path,
    tmp: &Path,
    entries: &[Entry],
    ops: &MoveOps,
    progress: &mut dyn FnMut(&MoveProgress),
) -> anyhow::Result<BTreeMap<PathBuf, u64>> {
    let mut p = MoveProgress { phase: "copying", done_bytes: 0, done_files: 0, ..done_progress(entries) };
    progress(&p);
    let mut copied = BTreeMap::new();
    for entry in entries {
        let (from, to) = (source.join(&entry.rel), tmp.join(&entry.rel));
        if entry.is_dir {
            std::fs::create_dir_all(&to).with_context(|| format!("couldn't create {}", to.display()))?;
            continue;
        }
        let before = p.done_bytes;
        let bytes = (ops.copy_file)(&from, &to, &mut |n| {
            p.done_bytes = before + n.min(entry.size);
            progress(&p);
        })
        .with_context(|| format!("couldn't copy {} to {}", from.display(), to.display()))?;
        copied.insert(entry.rel.clone(), bytes);
        p.done_bytes = before + entry.size;
        p.done_files += 1;
        progress(&p);
    }
    Ok(copied)
}

/// Every file that was copied is in the temp folder with the size that was
/// copied, nothing else is there, and (except log files, which keep growing
/// while DDMM runs) nothing changed in the source while it was copied.
fn verify(
    tmp: &Path,
    entries: &[Entry],
    copied: &BTreeMap<PathBuf, u64>,
    progress: &mut dyn FnMut(&MoveProgress),
) -> anyhow::Result<()> {
    progress(&MoveProgress { phase: "verifying", ..done_progress(entries) });
    let mut on_disk = BTreeMap::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for child in std::fs::read_dir(tmp.join(&rel))? {
            let child = child?;
            let child_rel = rel.join(child.file_name());
            let meta = std::fs::symlink_metadata(child.path())?;
            if meta.is_dir() {
                stack.push(child_rel);
            } else {
                on_disk.insert(child_rel, meta.len());
            }
        }
    }

    let expected_files = entries.iter().filter(|e| !e.is_dir).count();
    if on_disk.len() != expected_files || copied.len() != expected_files {
        bail!(
            "Verification failed: {} files were expected in the new folder but {} were found.",
            expected_files,
            on_disk.len()
        );
    }
    for entry in entries.iter().filter(|e| !e.is_dir) {
        let copied_size = copied.get(&entry.rel).copied();
        let found = on_disk.get(&entry.rel).copied();
        if found.is_none() || found != copied_size {
            bail!("Verification failed: {} wasn't copied completely.", entry.rel.display());
        }
        let is_log = entry.rel.components().next() == Some(Component::Normal("logs".as_ref()));
        if !is_log && copied_size != Some(entry.size) {
            bail!(
                "{} changed while it was being moved. Nothing was changed; try again.",
                entry.rel.display()
            );
        }
    }
    Ok(())
}

/// Rename `from` to `to`; if they turn out to be on different drives (a
/// mount point inside the destination), copy and remove instead.
fn move_into_place(from: &Path, to: &Path, ops: &MoveOps) -> anyhow::Result<()> {
    match (ops.rename)(from, to) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            log::info!("{from:?} and {to:?} are on different drives; copying instead of renaming");
            let result = copy_tree_plain(from, to, ops);
            if let Err(e) = result {
                let _ = remove_any(to);
                return Err(e);
            }
            remove_any(from).with_context(|| format!("couldn't remove {}", from.display()))
        }
        Err(e) => Err(e).with_context(|| format!("couldn't move {} to {}", from.display(), to.display())),
    }
}

fn copy_tree_plain(from: &Path, to: &Path, ops: &MoveOps) -> anyhow::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir(to).with_context(|| format!("couldn't create {}", to.display()))?;
        for child in std::fs::read_dir(from)? {
            let child = child?;
            copy_tree_plain(&child.path(), &to.join(child.file_name()), ops)?;
        }
    } else {
        let bytes = (ops.copy_file)(from, to, &mut |_| {}).with_context(|| format!("couldn't copy {}", from.display()))?;
        if bytes != meta.len() {
            bail!("{} wasn't copied completely", from.display());
        }
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// `path`, moved along with the data: `Some` when it's inside one of the
/// moved items of `old_base`.
pub fn rewrite_path(path: &Path, old_base: &Path, new_base: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    for name in MOVED_ITEMS {
        let item = old_base.join(name);
        match path_overlap(path, &item).ok()? {
            Some(PathOverlap::Same) => return Some(new_base.join(name)),
            Some(PathOverlap::FirstInsideSecond) => {
                let (p, i) = (crate::fs_util::resolve_path(path).ok()?, crate::fs_util::resolve_path(&item).ok()?);
                return Some(new_base.join(name).join(p.strip_prefix(&i).ok()?));
            }
            _ => {}
        }
    }
    None
}

/// Fix absolute paths in the moved `settings.json` that pointed into the
/// old data folder. Everything else DDMM stores is relative to the data
/// folder already (mod folders are found by scanning `mods/`, manifests use
/// relative paths, profiles hold only ids).
fn rewrite_stored_paths(old_base: &Path, new_base: &Path) -> anyhow::Result<()> {
    let file = new_base.join("settings.json");
    let Ok(data) = std::fs::read(&file) else { return Ok(()) };
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&data) else { return Ok(()) };
    let mut changed = false;
    if let Some(obj) = json.as_object_mut() {
        for key in ["DownloadsPath", "GamePath"] {
            let Some(old) = obj.get(key).and_then(|v| v.as_str()).map(PathBuf::from) else { continue };
            if let Some(new) = rewrite_path(&old, old_base, new_base) {
                log::info!("Updating {key} from {old:?} to {new:?}");
                obj.insert(key.to_string(), serde_json::Value::String(new.to_string_lossy().into_owned()));
                changed = true;
            }
        }
    }
    if changed {
        std::fs::write(&file, serde_json::to_vec_pretty(&json)?)
            .with_context(|| format!("couldn't update {}", file.display()))?;
    }
    Ok(())
}

/// Delete DDMM's items from the old data folder after a committed move.
/// Returns what couldn't be deleted (also recorded in [`CLEANUP_FILE`] in
/// the new folder, so the next start retries it). The old folder itself is
/// removed if that leaves it empty and it isn't the default location.
pub fn cleanup_old(plan: &MovePlan, default_path: &Path) -> Vec<PathBuf> {
    let mut left = Vec::new();
    for name in MOVED_ITEMS.iter().chain(REGENERATED_ITEMS) {
        let path = plan.source.join(name);
        if path.symlink_metadata().is_err() {
            continue;
        }
        if !matches!(path_overlap(&path, &plan.target), Ok(None)) {
            log::warn!("Not deleting {path:?}: it overlaps the new data folder");
            continue;
        }
        if let Err(e) = remove_any(&path) {
            log::warn!("Couldn't delete the old {path:?} yet: {e}");
            left.push(path);
        }
    }
    if !left.is_empty() {
        record_pending_cleanup(&plan.target, &left);
    } else if !matches!(path_overlap(&plan.source, default_path), Ok(Some(PathOverlap::Same)))
        && is_effectively_empty(&plan.source).unwrap_or(false)
    {
        for clutter in OS_CLUTTER {
            let _ = std::fs::remove_file(plan.source.join(clutter));
        }
        let _ = std::fs::remove_dir(&plan.source);
    }
    left
}

fn record_pending_cleanup(new_base: &Path, paths: &[PathBuf]) {
    let file = new_base.join(CLEANUP_FILE);
    match serde_json::to_vec_pretty(paths) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&file, data) {
                log::warn!("Couldn't record leftover old data in {file:?}: {e}");
            }
        }
        Err(e) => log::warn!("Couldn't record leftover old data: {e}"),
    }
}

/// Retry deleting old data a previous move couldn't (see [`cleanup_old`]).
/// Only ever deletes DDMM's own item names, and never anything overlapping
/// the current data folder.
pub fn run_pending_cleanup(base: &Path) {
    let file = base.join(CLEANUP_FILE);
    let Ok(data) = std::fs::read(&file) else { return };
    let paths: Vec<PathBuf> = serde_json::from_slice(&data).unwrap_or_default();
    let mut left = Vec::new();
    for path in paths {
        let known = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| MOVED_ITEMS.contains(&n) || REGENERATED_ITEMS.contains(&n));
        if !known || !matches!(path_overlap(&path, base), Ok(None)) {
            log::warn!("Ignoring unexpected leftover entry {path:?}");
            continue;
        }
        if path.symlink_metadata().is_err() {
            continue;
        }
        match remove_any(&path) {
            Ok(()) => log::info!("Deleted old data left over from a data folder move: {path:?}"),
            Err(e) => {
                log::warn!("Still couldn't delete {path:?}: {e}");
                left.push(path);
            }
        }
    }
    if left.is_empty() {
        let _ = std::fs::remove_file(&file);
    } else {
        record_pending_cleanup(base, &left);
    }
}

/// Free space (for this user) on the drive holding `path` (or its nearest
/// existing parent), if the OS reports it.
///
/// Asks about that one path only (`statvfs` / `GetDiskFreeSpaceExW`), never
/// by listing every mounted drive: querying an unrelated network or FUSE
/// mount can hang for a long time.
pub fn available_space(path: &Path) -> Option<u64> {
    let dir = nearest_existing_ancestor(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c_path` is a valid NUL-terminated string and `stat` is a
        // plain C struct that statvfs fills in.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        #[allow(clippy::useless_conversion)] // the field types differ by platform
        Some(u64::from(stat.f_bavail).saturating_mul(u64::from(stat.f_frsize)))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        // SAFETY: `wide` is NUL-terminated; the unused outputs may be null.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { None } else { Some(free) }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        None
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} bytes") } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests;
