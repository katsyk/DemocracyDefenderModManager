//! Folder names and deletes inside DDMM's mod storage (`<data>/mods`).
//!
//! Every install path (Add, drops, Add URL, the browser handoff and
//! extension, auto-import, Import, updates) turns a name it got from
//! somewhere else -- an archive's file name, a URL, a download's
//! `Content-Disposition` header, a folder's name -- into a folder under
//! `mods/`. [`safe_folder_name`] is the one place that decides what such a
//! folder may be called, and [`ensure_mod_folder`] is the check every
//! delete inside `mods/` goes through first, so a bad name can never make
//! DDMM delete or write anything outside a single mod's own folder.

use std::path::{Component, Path, PathBuf};

use anyhow::Context;

/// What a name with nothing usable left in it becomes.
pub const FALLBACK_NAME: &str = "mod";

/// Longest folder name DDMM creates (keeps paths comfortably short on
/// Windows).
pub const MAX_NAME_CHARS: usize = 80;

/// A single folder name, valid on every OS DDMM runs on, derived from
/// `name`:
///
/// - path separators, characters Windows forbids (`<>:"|?*`) and control
///   characters become `_`;
/// - leading dots and spaces, and trailing dots and spaces, are dropped
///   (so `.`, `..` and hidden-folder names can't come out of it);
/// - Windows' reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`,
///   `LPT1`-`LPT9`, with or without an extension) get a `mod ` prefix;
/// - it is cut to [`MAX_NAME_CHARS`] characters;
/// - nothing left becomes [`FALLBACK_NAME`].
///
/// The result is never empty and is always exactly one plain path
/// component. Applying it twice changes nothing.
pub fn safe_folder_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = trim_edges(&mapped);
    let mut out = if trimmed.is_empty() {
        FALLBACK_NAME.to_string()
    } else if is_reserved_windows_name(trimmed) {
        format!("{FALLBACK_NAME} {trimmed}")
    } else {
        trimmed.to_string()
    };
    if out.chars().count() > MAX_NAME_CHARS {
        let cut: String = out.chars().take(MAX_NAME_CHARS).collect();
        out = trim_edges(&cut).to_string();
        if out.is_empty() {
            out = FALLBACK_NAME.to_string();
        }
    }
    out
}

/// `name` without leading dots/whitespace and trailing dots/whitespace.
fn trim_edges(name: &str) -> &str {
    name.trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

/// Whether Windows treats `name` as a device (`CON`, `nul.txt`, `COM1.zip`,
/// ...), whatever follows the first dot.
pub fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    (prefix == "COM" || prefix == "LPT")
        && rest.len() == 1
        && (rest[0].is_ascii_digit() || matches!(rest[0], '¹' | '²' | '³'))
}

/// `wanted` if no folder `mods_root/wanted` exists yet, otherwise the first
/// free `wanted (2)`, `wanted (3)`, ... (existence checked without following
/// symlinks, and case-insensitively against `also_taken`). `wanted` is made
/// safe first.
pub fn free_folder_name(mods_root: &Path, wanted: &str, also_taken: &dyn Fn(&str) -> bool) -> String {
    let base = safe_folder_name(wanted);
    let free = |candidate: &str| !also_taken(candidate) && std::fs::symlink_metadata(mods_root.join(candidate)).is_err();
    if free(&base) {
        return base;
    }
    (2..).map(|n| numbered(&base, n)).find(|c| free(c)).unwrap()
}

/// `base (n)`, still within [`MAX_NAME_CHARS`].
pub fn numbered(base: &str, n: usize) -> String {
    let suffix = format!(" ({n})");
    let room = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
    let head: String = base.chars().take(room).collect();
    let head = trim_edges(&head);
    let head = if head.is_empty() { FALLBACK_NAME } else { head };
    format!("{head}{suffix}")
}

/// Check that `target` is exactly one folder directly inside `mods_root` --
/// `mods_root` joined with a single plain name (no `.`, `..`, root or
/// drive), whose parent resolves to the same folder as `mods_root` -- and
/// fail otherwise. Every delete inside the mod storage calls this first.
///
/// The same folder can be spelled differently on Windows: with or without
/// the `\\?\` prefix `canonicalize` adds, or in another letter case. Such a
/// `target` is accepted only when both its parent and `mods_root` resolve,
/// and resolve to the same folder -- the name check alone is never enough
/// for it.
pub fn ensure_mod_folder(mods_root: &Path, target: &Path) -> anyhow::Result<()> {
    let refuse = || {
        anyhow::anyhow!(
            "refusing to touch {:?}: it isn't a single mod folder inside DDMM's mod storage ({:?})",
            target,
            mods_root
        )
    };
    let lexically_inside = match target.strip_prefix(mods_root) {
        Ok(rest) => {
            let mut components = rest.components();
            match (components.next(), components.next()) {
                (Some(Component::Normal(name)), None) if !name.is_empty() => true,
                _ => return Err(refuse()),
            }
        }
        // Another spelling of the storage (see above): only a plain last
        // name, and no `.`/`..` anywhere, may go on to the resolved check.
        Err(_) => {
            let plain = target
                .components()
                .all(|c| !matches!(c, Component::CurDir | Component::ParentDir));
            if !plain || !matches!(target.components().next_back(), Some(Component::Normal(_))) {
                return Err(refuse());
            }
            false
        }
    };
    let name = target.file_name().ok_or_else(refuse)?;
    // Windows drops trailing dots and spaces from names, so a delete of
    // `Foo.` would remove `Foo`.
    if cfg!(windows) && has_windows_alias_ending(&name.to_string_lossy()) {
        return Err(refuse());
    }
    let parent = target.parent().ok_or_else(refuse)?;
    match (std::fs::canonicalize(parent), std::fs::canonicalize(mods_root)) {
        (Ok(canonical_parent), Ok(canonical_root)) => {
            // Both resolved the same way (prefix, on-disk letter case), so
            // one folder gives equal paths.
            if canonical_parent != canonical_root {
                return Err(refuse());
            }
        }
        // Some drives (certain network shares, RAM disks) can't be
        // canonicalized: the lexical check above -- `mods_root` plus one
        // plain name -- still holds, so rely on it rather than refusing
        // every delete there.
        (parent_result, root_result) if lexically_inside => {
            log::warn!(
                "Couldn't resolve {:?} / {:?} ({:?} / {:?}); relying on the name check alone.",
                parent,
                mods_root,
                parent_result.err(),
                root_result.err()
            );
        }
        _ => return Err(refuse()),
    }
    Ok(())
}

/// Whether `name` ends in a dot or a space, which Windows silently strips
/// (so it names a different folder there).
pub fn has_windows_alias_ending(name: &str) -> bool {
    name.ends_with('.') || name.ends_with(' ')
}

/// `remove_dir_all(target)`, but only after [`ensure_mod_folder`] agrees
/// that `target` is a single folder directly inside `mods_root`.
///
/// A folder that's already gone counts as removed. When the delete fails,
/// read-only attributes inside the folder are cleared (Windows won't
/// delete a read-only file on drives without POSIX deletes, such as FAT32
/// or exFAT) and it's tried again a few times, a moment apart (a virus
/// scanner or the search indexer often has a new file open briefly).
pub async fn remove_mod_folder(mods_root: &Path, target: &Path) -> anyhow::Result<()> {
    ensure_mod_folder(mods_root, target)?;
    let target_owned = target.to_path_buf();
    tokio::task::spawn_blocking(move || remove_tree_with_retries(&target_owned, REMOVE_RETRY_DELAYS))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|r| r)
        .with_context(|| format!("couldn't delete {:?}", target))
}

/// How long to wait before each retry of a failed delete.
pub const REMOVE_RETRY_DELAYS: &[std::time::Duration] = &[
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(300),
    std::time::Duration::from_millis(700),
];

/// Delete the tree at `target` (never following symlinks or junctions),
/// retrying after each of `delays` with read-only attributes cleared.
/// Already gone is success.
fn remove_tree_with_retries(target: &Path, delays: &[std::time::Duration]) -> anyhow::Result<()> {
    let mut last = match remove_tree_once(target) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    for delay in delays {
        log::info!("Deleting {:?} failed ({}); clearing read-only attributes and trying again.", target, last);
        clear_readonly_tree(target);
        std::thread::sleep(*delay);
        last = match remove_tree_once(target) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
    }
    Err(last.into())
}

fn remove_tree_once(target: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // `remove_dir_all` also says "not found" for a file that vanished
            // under it; only a missing folder is done.
            if std::fs::symlink_metadata(target).is_err() {
                Ok(())
            } else {
                Err(e)
            }
        }
        other => other,
    }
}

/// Make everything in the tree at `root` (itself included) writable, so it
/// can be deleted. Symlinks and junctions are neither changed nor
/// followed. Best effort: errors are ignored.
pub fn clear_readonly_tree(root: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(root) else { return };
    if meta.file_type().is_symlink() {
        return;
    }
    make_writable(root, &meta);
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                clear_readonly_tree(&entry.path());
            }
        }
    }
}

// On Windows `set_readonly(false)` only clears the read-only attribute.
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path, meta: &std::fs::Metadata) {
    let mut perms = meta.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(unix)]
fn make_writable(path: &Path, meta: &std::fs::Metadata) {
    use std::os::unix::fs::PermissionsExt;
    // Owner only: a folder also needs to be listable and enterable.
    let wanted = if meta.is_dir() { 0o700 } else { 0o200 };
    let mode = meta.permissions().mode();
    if mode & wanted != wanted {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | wanted));
    }
}

#[cfg(not(any(windows, unix)))]
fn make_writable(_path: &Path, _meta: &std::fs::Metadata) {}

/// Prefix of the folder a deleted mod's folder is renamed to before its
/// files are deleted. Taking the whole folder out of the storage in one
/// rename first means a mod is never left half deleted: either the rename
/// works and the mod is gone from the list at once (its files follow, at
/// the latest on the next start), or it fails and nothing was touched.
pub const PENDING_DELETE_PREFIX: &str = ".delete-";
/// Suffix of the small file written next to a pending delete before the
/// rename. Only a `.delete-<id>` folder that has one is ever deleted at
/// startup, so a folder DDMM didn't set aside itself is never removed.
pub const PENDING_DELETE_RECORD_SUFFIX: &str = ".pending";

/// The record file for the pending-delete folder `dir`.
pub fn pending_delete_record_path(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(PENDING_DELETE_RECORD_SUFFIX);
    dir.with_file_name(name)
}

/// Prefix of the throwaway folder an archive whose mod sits in a wrapper
/// folder (`ModName/manifest.json`) is extracted into; the wrapped folder
/// is then renamed into place. Always safe to delete: it only ever holds
/// a copy of an archive's contents.
pub const UNWRAP_PREFIX: &str = ".unwrap-";

/// Prefix of the throwaway folder an update is extracted into.
pub const UPDATE_STAGING_PREFIX: &str = ".update-";
/// Prefix of the folder the old version of a mod is set aside in while an
/// update is swapped in.
pub const UPDATE_BACKUP_PREFIX: &str = ".update-backup-";
/// Suffix of the small file next to an update backup that records which
/// folder the backup belongs in.
pub const UPDATE_BACKUP_RECORD_SUFFIX: &str = ".origin";

/// The record file for the update backup folder `backup_dir`.
pub fn backup_record_path(backup_dir: &Path) -> PathBuf {
    let mut name = backup_dir.file_name().unwrap_or_default().to_os_string();
    name.push(UPDATE_BACKUP_RECORD_SUFFIX);
    backup_dir.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_and_empty_names_fall_back() {
        for bad in ["", ".", "..", "...", " . ", ". .", "  ", "...."] {
            assert_eq!(safe_folder_name(bad), "mod", "{bad:?}");
        }
    }

    #[test]
    fn separators_and_traversal_become_one_component() {
        for bad in ["../x", "..\\x", "a/../../b", "/etc", "C:\\Windows", "\\\\server\\share"] {
            let safe = safe_folder_name(bad);
            let mut comps = Path::new(&safe).components();
            assert!(matches!(comps.next(), Some(Component::Normal(_))), "{bad:?} -> {safe:?}");
            assert!(comps.next().is_none(), "{bad:?} -> {safe:?}");
            assert!(!safe.contains(['/', '\\', ':']), "{bad:?} -> {safe:?}");
        }
    }

    #[test]
    fn leading_and_trailing_dots_and_spaces_go() {
        assert_eq!(safe_folder_name(".hidden"), "hidden");
        assert_eq!(safe_folder_name("..name.."), "name");
        assert_eq!(safe_folder_name(" name . "), "name");
        assert_eq!(safe_folder_name("a.b.c"), "a.b.c");
    }

    #[test]
    fn control_characters_are_replaced() {
        assert_eq!(safe_folder_name("a\nb\tc"), "a_b_c");
    }

    #[test]
    fn reserved_windows_names_get_a_prefix() {
        for name in ["CON", "con", "PRN", "AUX", "NUL", "nul.txt", "COM1", "com9.zip", "LPT1", "lpt5.tar.gz", "CON .txt", "COM¹"] {
            let safe = safe_folder_name(name);
            assert!(safe.starts_with("mod "), "{name:?} -> {safe:?}");
            assert!(!is_reserved_windows_name(&safe), "{name:?} -> {safe:?}");
        }
        for fine in ["CONSOLE", "COM10", "LPT", "Nullify", "com"] {
            assert_eq!(safe_folder_name(fine), fine);
        }
    }

    #[test]
    fn overlong_names_are_cut() {
        let safe = safe_folder_name(&"x".repeat(300));
        assert_eq!(safe.chars().count(), MAX_NAME_CHARS);
        let dotted = format!("{}.{}", "y".repeat(MAX_NAME_CHARS - 1), "z".repeat(10));
        assert!(!safe_folder_name(&dotted).ends_with('.'));
    }

    #[test]
    fn making_a_name_safe_twice_changes_nothing() {
        for name in ["..", ".x", "CON", "a/b", &"é".repeat(200), "ok name", " .COM1. "] {
            let once = safe_folder_name(name);
            assert_eq!(safe_folder_name(&once), once, "{name:?}");
        }
    }

    #[test]
    fn free_folder_name_numbers_existing_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Foo")).unwrap();
        std::fs::create_dir(dir.path().join("Foo (2)")).unwrap();
        assert_eq!(free_folder_name(dir.path(), "Foo", &|_| false), "Foo (3)");
        assert_eq!(free_folder_name(dir.path(), "Bar", &|_| false), "Bar");
        assert_eq!(free_folder_name(dir.path(), "Bar", &|c| c == "Bar"), "Bar (2)");
        assert_eq!(free_folder_name(dir.path(), "..", &|_| false), "mod");
        let long = numbered(&"x".repeat(MAX_NAME_CHARS), 12);
        assert!(long.chars().count() <= MAX_NAME_CHARS && long.ends_with(" (12)"));
    }

    #[test]
    fn only_single_folders_inside_the_storage_pass_the_guard() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        std::fs::create_dir_all(root.join("Foo")).unwrap();
        assert!(ensure_mod_folder(&root, &root.join("Foo")).is_ok());
        // Not yet existing is fine: only its parent must resolve.
        assert!(ensure_mod_folder(&root, &root.join("New")).is_ok());
        for bad in [
            root.clone(),
            root.join("."),
            root.join(".."),
            root.join("Foo").join("inner"),
            root.join("..").join("mods"),
            base.path().to_path_buf(),
            base.path().join("elsewhere"),
        ] {
            assert!(ensure_mod_folder(&root, &bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_windows_would_alias_are_detected() {
        for name in ["Foo.", "Foo ", "Foo. ", "..."] {
            assert!(has_windows_alias_ending(name), "{name:?}");
        }
        for name in ["Foo", "Foo.bar", ".Foo"] {
            assert!(!has_windows_alias_ending(name), "{name:?}");
        }
        // Nothing DDMM names itself ends that way.
        for name in ["Foo.", "Foo ", "a. . ."] {
            assert!(!has_windows_alias_ending(&safe_folder_name(name)), "{name:?}");
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_trailing_dot_name_never_deletes_its_alias_on_windows() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        std::fs::create_dir_all(root.join("Foo")).unwrap();
        assert!(remove_mod_folder(&root, &root.join("Foo.")).await.is_err());
        assert!(remove_mod_folder(&root, &root.join("Foo ")).await.is_err());
        assert!(root.join("Foo").is_dir());
    }

    /// When the storage can't be canonicalized (here: it doesn't exist),
    /// the one-name check alone decides instead of refusing everything.
    #[test]
    fn an_unresolvable_storage_falls_back_to_the_name_check() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("not-there");
        assert!(ensure_mod_folder(&root, &root.join("Foo")).is_ok());
        assert!(ensure_mod_folder(&root, &root.join("..")).is_err());
        assert!(ensure_mod_folder(&root, &root).is_err());
        assert!(ensure_mod_folder(&root, &root.join("a").join("b")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        let other = base.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&other, root.join("link")).unwrap();
        assert!(ensure_mod_folder(&root, &root.join("link").join("x")).is_err());
    }

    #[tokio::test]
    async fn remove_mod_folder_never_deletes_the_storage_or_above() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        std::fs::create_dir_all(root.join("Keep")).unwrap();
        std::fs::write(base.path().join("settings.json"), b"{}").unwrap();
        for bad in [root.join("."), root.join(".."), root.clone()] {
            assert!(remove_mod_folder(&root, &bad).await.is_err(), "{bad:?}");
        }
        assert!(root.join("Keep").is_dir());
        assert!(base.path().join("settings.json").is_file());
        remove_mod_folder(&root, &root.join("Keep")).await.unwrap();
        assert!(!root.join("Keep").exists());
    }

    /// The storage spelled another way (here: resolved, as `canonicalize`
    /// gives it) still passes for a folder directly in it -- and still
    /// refuses anything else.
    #[test]
    fn another_spelling_of_the_storage_passes_only_for_single_folders() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        std::fs::create_dir_all(root.join("Foo").join("inner")).unwrap();
        let resolved = std::fs::canonicalize(&root).unwrap();
        assert!(ensure_mod_folder(&root, &resolved.join("Foo")).is_ok());
        assert!(ensure_mod_folder(&resolved, &root.join("Foo")).is_ok());
        for bad in [
            resolved.clone(),
            resolved.join("Foo").join("inner"),
            resolved.join(".."),
            resolved.join("..").join("mods"),
            // (Not built on `resolved`: on Windows a `\\?\` path drops a
            // pushed `..` together with the name before it.)
            root.join("Foo").join("..").join("Bar"),
            std::fs::canonicalize(base.path()).unwrap().join("elsewhere"),
        ] {
            assert!(ensure_mod_folder(&root, &bad).is_err(), "{bad:?}");
        }
    }

    /// Another spelling that can't be resolved is refused: the name check
    /// alone only counts for `mods_root` itself plus one name.
    #[test]
    fn an_unresolvable_other_spelling_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        std::fs::create_dir_all(&root).unwrap();
        let other = base.path().join("not-there").join("mods");
        assert!(ensure_mod_folder(&root, &other.join("Foo")).is_err());
    }

    #[tokio::test]
    async fn a_folder_that_is_already_gone_counts_as_removed() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        std::fs::create_dir_all(&root).unwrap();
        remove_mod_folder(&root, &root.join("Gone")).await.unwrap();
    }

    #[test]
    fn clearing_read_only_makes_a_tree_deletable() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("Foo");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("sub").join("a.patch_0");
        std::fs::write(&file, b"x").unwrap();
        for p in [&file, &dir.join("sub")] {
            let mut perms = std::fs::metadata(p).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(p, perms).unwrap();
        }
        clear_readonly_tree(&dir);
        assert!(!std::fs::metadata(&file).unwrap().permissions().readonly());
        assert!(!std::fs::metadata(dir.join("sub")).unwrap().permissions().readonly());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A link inside a mod's folder pointing outside the storage: deleting
    /// the mod removes the link, never what it points to (and clearing
    /// read-only never follows it).
    #[tokio::test]
    async fn a_link_inside_a_mod_never_takes_its_target_with_it() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(root.join("Foo")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let precious = outside.join("precious.txt");
        std::fs::write(&precious, b"keep").unwrap();
        let mut perms = std::fs::metadata(&precious).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&precious, perms).unwrap();
        let link = root.join("Foo").join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        {
            // A junction: needs no special rights, unlike a symlink.
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .status()
                .unwrap();
            assert!(status.success());
        }
        clear_readonly_tree(&root.join("Foo"));
        assert!(std::fs::metadata(&precious).unwrap().permissions().readonly(), "the link was followed");
        remove_mod_folder(&root, &root.join("Foo")).await.unwrap();
        assert!(!root.join("Foo").exists());
        assert_eq!(std::fs::read(&precious).unwrap(), b"keep");
    }

    /// Windows: `canonicalize` gives `\\?\C:\...`, the storage path DDMM
    /// builds doesn't have the prefix; letter case can differ too. Both
    /// spellings are one folder and delete; the guard still refuses
    /// anything but a single folder in either spelling.
    #[cfg(windows)]
    #[tokio::test]
    async fn verbatim_and_other_case_spellings_work_on_windows() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        for name in ["A", "B", "C"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("x.patch_0"), b"x").unwrap();
        }
        let verbatim = std::fs::canonicalize(&root).unwrap();
        assert!(verbatim.to_string_lossy().starts_with(r"\\?\"), "{verbatim:?}");
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
        assert!(ensure_mod_folder(&root, &verbatim.join("A")).is_ok());
        assert!(ensure_mod_folder(&verbatim, &root.join("A")).is_ok());
        assert!(ensure_mod_folder(&root, &upper.join("A")).is_ok());
        for bad in [verbatim.join("A").join("x.patch_0"), verbatim.clone(), verbatim.join(".."), upper.join("A").join("x.patch_0")] {
            assert!(ensure_mod_folder(&root, &bad).is_err(), "{bad:?}");
        }
        // A trailing-dot alias is refused in the verbatim spelling too.
        assert!(ensure_mod_folder(&root, &verbatim.join("A.")).is_err());
        remove_mod_folder(&root, &verbatim.join("A")).await.unwrap();
        remove_mod_folder(&verbatim, &root.join("B")).await.unwrap();
        remove_mod_folder(&root, &upper.join("C")).await.unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }

    /// Windows: a read-only file (attribute set, as RAR archives and
    /// copied folders can bring along) inside a mod's folder is deleted.
    #[cfg(windows)]
    #[tokio::test]
    async fn read_only_files_are_deleted_on_windows() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("mods");
        let dir = root.join("Foo");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in [dir.join("manifest.json"), dir.join("sub").join("a.patch_0")] {
            std::fs::write(&f, b"x").unwrap();
            let mut perms = std::fs::metadata(&f).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&f, perms).unwrap();
        }
        remove_mod_folder(&root, &dir).await.unwrap();
        assert!(!dir.exists());
    }
}
