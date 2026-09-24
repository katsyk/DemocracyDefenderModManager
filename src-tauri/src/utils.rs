use std::path::{Path, PathBuf};

/// Recursively copy the contents of `src` into `dst`, creating directories
/// as needed. Symlinks are never followed (skipped outright); entries whose
/// file name matches one of `skip_names` are skipped at every depth.
pub async fn copy_dir_recursive(src: &Path, dst: &Path, skip_names: &[&str]) -> anyhow::Result<()> {
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];

    while let Some((from, to)) = stack.pop() {
        tokio::fs::create_dir_all(&to).await?;

        let mut entries = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;

            // Never follow symlinks.
            if file_type.is_symlink() {
                continue;
            }

            let name = entry.file_name();
            if skip_names
                .iter()
                .any(|skip| name.to_str() == Some(*skip))
            {
                continue;
            }

            let from_path = entry.path();
            let to_path = to.join(&name);

            if file_type.is_dir() {
                stack.push((from_path, to_path));
            } else if file_type.is_file() {
                tokio::fs::copy(&from_path, &to_path).await?;
            }
        }
    }

    Ok(())
}

pub async fn fix_path_casing(base: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    let mut current = base.to_path_buf();
    'components: for components in relative.components() {
        let component_str = components.as_os_str().to_string_lossy();
        
        let mut entries = tokio::fs::read_dir(&current).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name().to_string_lossy().to_lowercase() == component_str.to_lowercase() {
                current.push(entry.file_name());
                continue 'components;
            }
        }

        current.push(components.as_os_str());
    }

    Ok(current.strip_prefix(base)?.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copy_dir_recursive_copies_nested_files() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        tokio::fs::write(src.path().join("manifest.json"), b"{}")
            .await
            .unwrap();
        tokio::fs::create_dir(src.path().join("options")).await.unwrap();
        tokio::fs::write(src.path().join("options/a.txt"), b"a")
            .await
            .unwrap();

        copy_dir_recursive(src.path(), dst.path(), &[]).await.unwrap();

        assert!(dst.path().join("manifest.json").is_file());
        assert!(dst.path().join("options/a.txt").is_file());
        assert_eq!(
            tokio::fs::read_to_string(dst.path().join("options/a.txt"))
                .await
                .unwrap(),
            "a"
        );
    }

    #[tokio::test]
    async fn copy_dir_recursive_skips_named_entries() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        tokio::fs::write(src.path().join("manifest.json"), b"{}")
            .await
            .unwrap();
        tokio::fs::write(src.path().join(".hd2mm-origin.json"), b"{}")
            .await
            .unwrap();

        copy_dir_recursive(src.path(), dst.path(), &[".hd2mm-origin.json"])
            .await
            .unwrap();

        assert!(dst.path().join("manifest.json").is_file());
        assert!(!dst.path().join(".hd2mm-origin.json").exists());
    }

    #[tokio::test]
    async fn copy_dir_recursive_does_not_follow_symlinks() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        tokio::fs::write(outside.path().join("secret.txt"), b"secret")
            .await
            .unwrap();

        #[cfg(unix)]
        {
            tokio::fs::symlink(outside.path().join("secret.txt"), src.path().join("link.txt"))
                .await
                .unwrap();

            copy_dir_recursive(src.path(), dst.path(), &[]).await.unwrap();

            assert!(!dst.path().join("link.txt").exists());
        }
    }
}