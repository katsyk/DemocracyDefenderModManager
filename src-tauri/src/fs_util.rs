//! Filesystem helpers that behave the same on every platform DDMM ships
//! for.
//!
//! The main one is [`move_path`]: `rename(2)` cannot move anything across
//! filesystems -- it fails with `EXDEV` ("Invalid cross-device link") on
//! Linux/macOS and `ERROR_NOT_SAME_DEVICE` on Windows. On a typical Arch /
//! CachyOS box that is easy to hit: `/tmp` is a tmpfs, `~` is btrfs/ext4,
//! and the Steam library is often on a second drive (another btrfs volume,
//! or NTFS/exFAT for dual-booters). Every move DDMM makes goes through here
//! so it falls back to copy + remove instead of failing.

use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::Context;

/// Whether `err` is the "source and destination are on different
/// filesystems" error `rename` returns.
pub fn is_cross_device(err: &io::Error) -> bool {
    if err.kind() == io::ErrorKind::CrossesDevices {
        return true;
    }
    match err.raw_os_error() {
        #[cfg(unix)]
        Some(code) => code == libc_exdev(),
        // ERROR_NOT_SAME_DEVICE
        #[cfg(windows)]
        Some(code) => code == 17,
        #[cfg(not(any(unix, windows)))]
        Some(_) => false,
        None => false,
    }
}

#[cfg(unix)]
const fn libc_exdev() -> i32 {
    // EXDEV is 18 on Linux, macOS and the BSDs.
    18
}

/// Move `from` to `to` (a file or a whole directory tree), like
/// `tokio::fs::rename`, but falling back to a recursive copy followed by
/// removing the source when the two paths are on different filesystems.
///
/// `to` must not already exist. Errors name both paths so they are useful
/// when shown to the user.
pub async fn move_path(from: &Path, to: &Path) -> anyhow::Result<()> {
    move_path_with(from, to, |a, b| {
        let (a, b) = (a.to_path_buf(), b.to_path_buf());
        async move { tokio::fs::rename(a, b).await }
    })
    .await
}

/// [`move_path`] with the `rename` step injectable, so tests can force the
/// cross-device fallback without needing two real filesystems.
pub(crate) async fn move_path_with<F, Fut>(from: &Path, to: &Path, rename: F) -> anyhow::Result<()>
where
    F: Fn(&Path, &Path) -> Fut,
    Fut: std::future::Future<Output = io::Result<()>>,
{
    match rename(from, to).await {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            log::info!(
                "{:?} and {:?} are on different filesystems; copying instead of renaming",
                from, to
            );
            copy_then_remove(from, to).await
        }
        Err(e) => Err(e).with_context(|| format!("failed to move {:?} to {:?}", from, to)),
    }
}

async fn copy_then_remove(from: &Path, to: &Path) -> anyhow::Result<()> {
    let meta = tokio::fs::symlink_metadata(from)
        .await
        .with_context(|| format!("failed to read {:?}", from))?;

    if meta.is_dir() {
        let copied = copy_tree(from, to).await;
        if let Err(e) = copied {
            // Don't leave a half-copied tree behind at the destination.
            let _ = tokio::fs::remove_dir_all(to).await;
            return Err(e).with_context(|| format!("failed to copy {:?} to {:?}", from, to));
        }
        tokio::fs::remove_dir_all(from)
            .await
            .with_context(|| format!("copied {:?} to {:?} but failed to remove the original", from, to))?;
    } else {
        if let Err(e) = tokio::fs::copy(from, to).await {
            let _ = tokio::fs::remove_file(to).await;
            return Err(e).with_context(|| format!("failed to copy {:?} to {:?}", from, to));
        }
        tokio::fs::remove_file(from)
            .await
            .with_context(|| format!("copied {:?} to {:?} but failed to remove the original", from, to))?;
    }
    Ok(())
}

/// Recursive copy that, unlike `utils::copy_dir_recursive`, keeps
/// everything (no skip list) and refuses to overwrite: `to` must not exist.
async fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    // Checked before the existence test so a move of a folder into its own
    // subtree reports the real problem, not "already exists".
    ensure_no_overlap(from, to)?;
    if tokio::fs::try_exists(to).await.unwrap_or(false) {
        anyhow::bail!("destination {:?} already exists", to);
    }
    crate::utils::copy_dir_recursive(from, to, &[]).await
}

/// How two paths relate on disk once symlinks, `.`/`..`, and (on Windows
/// and macOS) letter case are resolved. See [`path_overlap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOverlap {
    /// Both paths name the same folder.
    Same,
    /// The first path is somewhere inside the second.
    FirstInsideSecond,
    /// The second path is somewhere inside the first.
    SecondInsideFirst,
}

/// Resolve `path` to an absolute path with no symlinks and no `.`/`..`,
/// even when (the tail of) it doesn't exist yet: the longest existing
/// prefix is canonicalized (which also gives it its on-disk letter case on
/// Windows), and the missing rest is appended with `..` applied lexically.
pub fn resolve_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    let mut exists = true;
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                // `resolved` has no symlinks left in it, so its lexical
                // parent is its real parent.
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                if exists {
                    match std::fs::canonicalize(&resolved) {
                        Ok(canonical) => resolved = canonical,
                        // Missing (or unreadable): keep the rest as written.
                        Err(_) => exists = false,
                    }
                }
            }
        }
    }
    Ok(resolved)
}

/// Whether two already-[`resolve_path`]d paths name the same folder.
fn same_resolved(a: &Path, b: &Path) -> bool {
    let (mut ca, mut cb) = (a.components(), b.components());
    let names_equal = loop {
        match (ca.next(), cb.next()) {
            (None, None) => break true,
            (Some(x), Some(y)) if component_eq(x.as_os_str(), y.as_os_str()) => continue,
            _ => break false,
        }
    };
    // File identity (device + inode, or volume + file index) catches the
    // aliases a name comparison can't: bind mounts, 8.3 short names, a
    // case-insensitive macOS volume, a second drive letter for one volume.
    names_equal || same_file::is_same_file(a, b).unwrap_or(false)
}

fn component_eq(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if a == b {
        return true;
    }
    // Windows and (by default) macOS filesystems ignore letter case. Only
    // the not-yet-existing tail of a path can still differ in case here
    // (existing parts were canonicalized); treating those as equal only
    // ever makes the overlap check stricter, never looser.
    if cfg!(any(windows, target_os = "macos")) {
        return a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase();
    }
    false
}

/// Whether resolved path `child` is `parent` or anywhere below it.
fn is_within(child: &Path, parent: &Path) -> bool {
    child.ancestors().any(|ancestor| same_resolved(ancestor, parent))
}

/// Answer "are `a` and `b` the same folder, or is one inside the other?"
/// after resolving symlinks, `..`, and letter case on case-insensitive
/// platforms. Neither path has to exist. `None` means they don't overlap.
///
/// Every copy, move and import of a folder goes through this: copying a
/// folder into its own subtree re-reads what it just wrote and nests the
/// folder inside itself over and over until the disk fills up.
pub fn path_overlap(a: &Path, b: &Path) -> io::Result<Option<PathOverlap>> {
    let (a, b) = (resolve_path(a)?, resolve_path(b)?);
    Ok(match (is_within(&a, &b), is_within(&b, &a)) {
        (true, true) => Some(PathOverlap::Same),
        (true, false) => Some(PathOverlap::FirstInsideSecond),
        (false, true) => Some(PathOverlap::SecondInsideFirst),
        (false, false) => None,
    })
}

/// Fail with a readable error if copying/moving `src` to `dst` would put
/// one inside the other (see [`path_overlap`]).
pub fn ensure_no_overlap(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let overlap = path_overlap(src, dst)
        .with_context(|| format!("couldn't check whether {:?} and {:?} overlap", src, dst))?;
    match overlap {
        None => Ok(()),
        Some(PathOverlap::Same) => {
            anyhow::bail!("can't copy {:?} onto itself ({:?} is the same folder)", src, dst)
        }
        Some(PathOverlap::SecondInsideFirst) => anyhow::bail!(
            "can't copy {:?} into {:?}: the destination is inside the folder being copied, \
             so it would copy itself into itself over and over",
            src, dst
        ),
        Some(PathOverlap::FirstInsideSecond) => anyhow::bail!(
            "can't copy {:?} into {:?}: the folder being copied is inside the destination",
            src, dst
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn recognizes_cross_device_error() {
        assert!(is_cross_device(&exdev()));
        assert!(is_cross_device(&io::Error::from(io::ErrorKind::CrossesDevices)));
        assert!(!is_cross_device(&io::Error::from(io::ErrorKind::NotFound)));
    }

    #[tokio::test]
    async fn same_filesystem_move_just_renames() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.zip");
        let to = dir.path().join("b.zip");
        tokio::fs::write(&from, b"data").await.unwrap();

        move_path(&from, &to).await.unwrap();

        assert!(!from.exists());
        assert_eq!(tokio::fs::read(&to).await.unwrap(), b"data");
    }

    #[tokio::test]
    async fn forced_cross_device_file_move_falls_back_to_copy() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let from = src.path().join("mod.zip");
        let to = dst.path().join("mod.zip");
        tokio::fs::write(&from, b"zipbytes").await.unwrap();

        move_path_with(&from, &to, |_, _| async { Err(exdev()) }).await.unwrap();

        assert!(!from.exists(), "source must be removed after the copy");
        assert_eq!(tokio::fs::read(&to).await.unwrap(), b"zipbytes");
    }

    #[tokio::test]
    async fn forced_cross_device_dir_move_copies_whole_tree() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let from = src.path().join("MyMod");
        let to = dst.path().join("MyMod");
        tokio::fs::create_dir_all(from.join("Options/Red")).await.unwrap();
        tokio::fs::write(from.join("manifest.json"), b"{}").await.unwrap();
        tokio::fs::write(from.join("Options/Red/0123456789abcdef.patch_0"), b"p").await.unwrap();

        move_path_with(&from, &to, |_, _| async { Err(exdev()) }).await.unwrap();

        assert!(!from.exists());
        assert!(to.join("manifest.json").is_file());
        assert_eq!(
            tokio::fs::read(to.join("Options/Red/0123456789abcdef.patch_0")).await.unwrap(),
            b"p"
        );
    }

    #[test]
    fn overlap_same_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(path_overlap(dir.path(), dir.path()).unwrap(), Some(PathOverlap::Same));
    }

    #[test]
    fn overlap_destination_inside_source_even_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("mods").join("not-created-yet");
        assert_eq!(path_overlap(dir.path(), &inside).unwrap(), Some(PathOverlap::SecondInsideFirst));
        assert_eq!(path_overlap(&inside, dir.path()).unwrap(), Some(PathOverlap::FirstInsideSecond));
    }

    #[test]
    fn overlap_ignores_siblings_that_share_a_name_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        std::fs::create_dir_all(dir.path().join("mods2")).unwrap();
        assert_eq!(path_overlap(&dir.path().join("mods"), &dir.path().join("mods2")).unwrap(), None);
        assert_eq!(path_overlap(&dir.path().join("mods"), &dir.path().join("mods2/x")).unwrap(), None);
    }

    #[test]
    fn overlap_resolves_dot_and_dotdot_segments() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(mods.join("ModA")).unwrap();
        let roundabout = mods.join("ModA").join("..").join(".");
        assert_eq!(path_overlap(&roundabout, &mods).unwrap(), Some(PathOverlap::Same));
        // `..` in a part that doesn't exist yet is applied too.
        let missing = mods.join("new").join("..").join("other");
        assert_eq!(path_overlap(&mods, &missing).unwrap(), Some(PathOverlap::SecondInsideFirst));
        let outside = mods.join("..").join("elsewhere");
        assert_eq!(path_overlap(&mods, &outside).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn overlap_sees_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("mods")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(path_overlap(&link, &real).unwrap(), Some(PathOverlap::Same));
        assert_eq!(path_overlap(&link, &real.join("mods/new")).unwrap(), Some(PathOverlap::SecondInsideFirst));
        assert_eq!(path_overlap(&real, &link.join("mods")).unwrap(), Some(PathOverlap::SecondInsideFirst));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn overlap_ignores_letter_case_on_case_insensitive_platforms() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let upper = dir.path().join("MODS");
        assert_eq!(path_overlap(&upper, &mods).unwrap(), Some(PathOverlap::Same));
        assert_eq!(path_overlap(&mods, &upper.join("Missing")).unwrap(), Some(PathOverlap::SecondInsideFirst));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn overlap_respects_letter_case_on_linux() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        std::fs::create_dir_all(dir.path().join("MODS")).unwrap();
        assert_eq!(path_overlap(&dir.path().join("MODS"), &dir.path().join("mods")).unwrap(), None);
    }

    #[tokio::test]
    async fn cross_device_move_into_own_subtree_is_refused_and_source_kept() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("MyMod");
        tokio::fs::create_dir_all(&from).await.unwrap();
        tokio::fs::write(from.join("0123456789abcdef.patch_0"), b"p").await.unwrap();
        let to = from.join("nested");

        let err = move_path_with(&from, &to, |_, _| async { Err(exdev()) }).await.unwrap_err();
        assert!(format!("{err:#}").contains("inside"), "{err:#}");
        assert!(from.join("0123456789abcdef.patch_0").is_file());
        assert!(!to.exists());
    }

    #[tokio::test]
    async fn other_rename_errors_are_reported_with_both_paths() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("missing");
        let to = dir.path().join("dest");

        let err = move_path(&from, &to).await.unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("missing") && msg.contains("dest"), "{msg}");
    }

    /// A real cross-filesystem move, not an injected error: `/dev/shm` is a
    /// tmpfs on Linux, so if the system temp dir lives on a different
    /// filesystem (the usual case outside CachyOS/Arch, where `/tmp` is
    /// itself tmpfs) `rename` genuinely fails with EXDEV here. Skips itself
    /// when the two happen to share a device.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn real_cross_filesystem_move_between_tmp_and_dev_shm() {
        use std::os::unix::fs::MetadataExt;

        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let a = tempfile::tempdir().unwrap();
        let b = match tempfile::tempdir_in(shm) {
            Ok(b) => b,
            Err(_) => return,
        };
        if std::fs::metadata(a.path()).unwrap().dev() == std::fs::metadata(b.path()).unwrap().dev() {
            eprintln!("temp dir and /dev/shm share a filesystem; skipping");
            return;
        }

        let from = a.path().join("tree");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("sub/file.patch_0"), b"x").unwrap();
        let to = b.path().join("tree");

        // Plain rename really does fail across devices...
        let raw = std::fs::rename(&from, &to).unwrap_err();
        assert!(is_cross_device(&raw), "expected EXDEV, got {raw:?}");

        // ...and move_path handles it.
        move_path(&from, &to).await.unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(to.join("sub/file.patch_0")).unwrap(), b"x");
    }
}
