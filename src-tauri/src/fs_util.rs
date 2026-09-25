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
use std::path::Path;

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
    if tokio::fs::try_exists(to).await.unwrap_or(false) {
        anyhow::bail!("destination {:?} already exists", to);
    }
    crate::utils::copy_dir_recursive(from, to, &[]).await
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
