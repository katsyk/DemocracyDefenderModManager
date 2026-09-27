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
pub fn ensure_mod_folder(mods_root: &Path, target: &Path) -> anyhow::Result<()> {
    let refuse = || {
        anyhow::anyhow!(
            "refusing to touch {:?}: it isn't a single mod folder inside DDMM's mod storage ({:?})",
            target,
            mods_root
        )
    };
    let rest = target.strip_prefix(mods_root).map_err(|_| refuse())?;
    let mut components = rest.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if !name.is_empty() => {
            // Windows drops trailing dots and spaces from names, so a
            // delete of `Foo.` would remove `Foo`.
            if cfg!(windows) && has_windows_alias_ending(&name.to_string_lossy()) {
                return Err(refuse());
            }
        }
        _ => return Err(refuse()),
    }
    let parent = target.parent().ok_or_else(refuse)?;
    match (std::fs::canonicalize(parent), std::fs::canonicalize(mods_root)) {
        (Ok(canonical_parent), Ok(canonical_root)) => {
            if canonical_parent != canonical_root {
                return Err(refuse());
            }
        }
        // Some drives (certain network shares, RAM disks) can't be
        // canonicalized: the lexical check above -- `mods_root` plus one
        // plain name -- still holds, so rely on it rather than refusing
        // every delete there.
        (parent_result, root_result) => {
            log::warn!(
                "Couldn't resolve {:?} / {:?} ({:?} / {:?}); relying on the name check alone.",
                parent,
                mods_root,
                parent_result.err(),
                root_result.err()
            );
        }
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
pub async fn remove_mod_folder(mods_root: &Path, target: &Path) -> anyhow::Result<()> {
    ensure_mod_folder(mods_root, target)?;
    tokio::fs::remove_dir_all(target)
        .await
        .with_context(|| format!("couldn't delete {:?}", target))
}

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
}
