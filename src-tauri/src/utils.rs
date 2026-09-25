use std::{path::{Path, PathBuf}, sync::OnceLock};

use regex::Regex;

static PATCH_FILE_REGEX: OnceLock<Regex> = OnceLock::new();

/// Whether a bare file name looks like a Helldivers 2 patch file
/// (`<16 hex chars>.patch_<n>`, optionally `.gpu_resources` or `.stream`).
/// Shared between deploy's file grouping (`commands/mod.rs`) and archive
/// layout auto-detection (below).
pub fn is_patch_filename(name: &str) -> bool {
    PATCH_FILE_REGEX
        .get_or_init(|| Regex::new(r"^[0-9a-f]{16}\.patch_\d+(?:\.gpu_resources|\.stream)?$").unwrap())
        .is_match(name)
}

/// The result of scanning a freshly-installed, manifest-less mod directory
/// for where its Helldivers 2 patch files actually live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchLayout {
    /// The mod root directly contains patch files -- the layout the manager
    /// has always assumed.
    RootHasFiles,
    /// One or more directories (up to 4 levels deep, relative paths using
    /// `/`, naturally sorted) directly contain patch files, but the root
    /// doesn't. Covers both a single wrapper folder (`ModName/`) and
    /// several variant folders (`Red/`, `Blue/`, ...) -- either way the
    /// generated legacy manifest's `Options` list is set to these paths, so
    /// the existing option picker (already defaulting to index 0) handles
    /// both cases without new UI.
    Options(Vec<String>),
    /// No patch files found anywhere in the first 4 levels.
    NoneFound,
}

/// Scan `mod_dir` for Helldivers 2 patch files: first the root itself, then
/// (if the root has none) every directory up to 4 levels deep that
/// *directly* contains at least one. A qualifying directory is not
/// recursed into further -- its contents are that option's file tree, not a
/// place to look for more nested variants. Symlinked directories are never
/// followed.
pub async fn detect_patch_layout(mod_dir: &Path) -> anyhow::Result<PatchLayout> {
    if dir_has_patch_files(mod_dir).await? {
        return Ok(PatchLayout::RootHasFiles);
    }

    let mut found = Vec::new();
    collect_patch_dirs(mod_dir, mod_dir, 1, 4, &mut found).await?;

    if found.is_empty() {
        Ok(PatchLayout::NoneFound)
    } else {
        found.sort_by(|a, b| natural_cmp(a, b));
        Ok(PatchLayout::Options(found))
    }
}

async fn dir_has_patch_files(dir: &Path) -> anyhow::Result<bool> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_file() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            if is_patch_filename(name) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn collect_patch_dirs<'a>(
    root: &'a Path,
    dir: &'a Path,
    depth: u32,
    max_depth: u32,
    found: &'a mut Vec<String>,
) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
    Box::pin(async move {
        let mut subdirs = Vec::new();
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            // Never follow symlinks while searching.
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }
            subdirs.push(entry.path());
        }

        for sub in subdirs {
            if dir_has_patch_files(&sub).await? {
                let rel = sub.strip_prefix(root)?;
                found.push(path_to_rel_string(rel));
            } else if depth < max_depth {
                collect_patch_dirs(root, &sub, depth + 1, max_depth, found).await?;
            }
        }

        Ok(())
    })
}

/// Render a relative path as a `/`-separated string regardless of the host
/// platform's native separator, so generated manifests are portable.
fn path_to_rel_string(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Compare two strings "naturally": runs of ASCII digits compare by numeric
/// value rather than character-by-character, so `"Option 2"` sorts before
/// `"Option 10"`. Non-digit runs compare case-insensitively.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_number(&mut ai);
                    let nb = take_number(&mut bi);
                    match na.cmp(&nb) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                            continue;
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(&c) = it.peek() {
        if let Some(d) = c.to_digit(10) {
            n = n.saturating_mul(10).saturating_add(d as u64);
            it.next();
        } else {
            break;
        }
    }
    n
}

/// How many folders deep [`copy_dir_recursive`] will go before giving up.
/// Real mods are a handful of levels deep; this is only a backstop so a
/// runaway copy can never fill a disk.
pub const MAX_COPY_DEPTH: usize = 64;

/// Recursively copy the contents of `src` into `dst`, creating directories
/// as needed. Symlinks (and Windows junctions) are never followed (skipped
/// outright); entries whose file name matches one of `skip_names` are
/// skipped at every depth.
///
/// Refuses up front when `src` and `dst` are the same folder or one is
/// inside the other ([`crate::fs_util::ensure_no_overlap`]): copying a
/// folder into its own subtree re-reads what it just wrote and nests
/// copies of itself inside itself until the path gets too long or the disk
/// is full. As further backstops, each folder's listing is taken before
/// anything is written into its copy, the destination folder itself is
/// never descended into, and nothing deeper than [`MAX_COPY_DEPTH`] is
/// copied.
///
/// On failure, whatever was already copied is left in `dst`; every caller
/// removes `dst` again (it's always a folder the caller just created).
pub async fn copy_dir_recursive(src: &Path, dst: &Path, skip_names: &[&str]) -> anyhow::Result<()> {
    crate::fs_util::ensure_no_overlap(src, dst)?;

    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf(), 0usize)];

    while let Some((from, to, depth)) = stack.pop() {
        if depth > MAX_COPY_DEPTH {
            anyhow::bail!(
                "{:?} has folders nested more than {} levels deep; refusing to copy it",
                src,
                MAX_COPY_DEPTH
            );
        }

        // Snapshot the listing before creating anything under `to`.
        let mut listing = Vec::new();
        let mut entries = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            listing.push((entry.path(), entry.file_name(), entry.file_type().await?));
        }

        tokio::fs::create_dir_all(&to).await?;

        for (from_path, name, file_type) in listing {
            // Never follow symlinks.
            if file_type.is_symlink() {
                continue;
            }

            if skip_names
                .iter()
                .any(|skip| name.to_str() == Some(*skip))
            {
                continue;
            }

            let to_path = to.join(&name);

            if file_type.is_dir() {
                if same_file::is_same_file(&from_path, dst).unwrap_or(false) {
                    log::warn!("Not copying {:?}: it is the copy's own destination", from_path);
                    continue;
                }
                stack.push((from_path, to_path, depth + 1));
            } else if file_type.is_file() {
                tokio::fs::copy(&from_path, &to_path).await?;
            }
        }
    }

    Ok(())
}

/// Resolve a manifest-relative path (an icon, an option image, an option's
/// `Include` folder) against the files that actually exist under `base`,
/// returning the relative path with each component's real on-disk casing.
///
/// Manifests are mostly written on Windows, where neither casing nor the
/// separator matter. On Linux both do, so this, on every platform:
/// - treats `\` as a separator (`"Options\Red"` means `Options/Red`),
/// - matches each component case-insensitively and returns the name as it
///   is on disk (`options/red` gives `Options/Red`; an exact match wins
///   if a case-sensitive filesystem has both),
/// - keeps only plain name components: `.`, `..`, roots, and Windows
///   drive/UNC/verbatim prefixes (`C:`, `C:foo`, `\\server\share`,
///   `\\?\C:\`) are dropped, so the result is always relative and
///   `base.join(result)` always stays inside `base`,
/// - never fails just because something is missing: from the first
///   component that doesn't exist on disk, the rest are kept as written,
///   and the caller's own "not found" error names the path.
pub async fn fix_path_casing(base: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    let normalized = relative.to_string_lossy().replace('\\', "/");

    let mut result = PathBuf::new();
    let mut matching = true;
    for piece in normalized.split('/') {
        // Drop a drive prefix (`C:` / drive-relative `C:foo`) the same way
        // on every platform.
        let b = piece.as_bytes();
        let piece = if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' { &piece[2..] } else { piece };
        // Then let the platform's own parser classify what's left and keep
        // only plain names -- pushing anything else (a root, or a Windows
        // prefix) would replace the whole path.
        for component in Path::new(piece).components() {
            let std::path::Component::Normal(name) = component else { continue };
            let name = name.to_string_lossy();

            if matching {
                match find_entry_case_insensitive(&base.join(&result), &name).await {
                    Some(real) => {
                        result.push(real);
                        continue;
                    }
                    None => matching = false,
                }
            }
            result.push(name.as_ref());
        }
    }

    Ok(result)
}

/// The real name of the entry in `dir` matching `name` ignoring case,
/// preferring an exact match.
async fn find_entry_case_insensitive(dir: &Path, name: &str) -> Option<std::ffi::OsString> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    let wanted = name.to_lowercase();
    let mut variant = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let entry_name = entry.file_name();
        let lossy = entry_name.to_string_lossy();
        if lossy == name {
            return Some(entry_name);
        }
        if variant.is_none() && lossy.to_lowercase() == wanted {
            variant = Some(entry_name);
        }
    }
    variant
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Component-wise comparison (so `/` vs `\\` never matters), strict
    /// about case: `fix_path_casing` returns on-disk casing on every
    /// platform, Windows included.
    fn assert_components(actual: &Path, expected: &[&str]) {
        let got: Vec<String> = actual
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, expected, "{:?}", actual);
    }

    #[tokio::test]
    async fn fix_path_casing_handles_backslashes_and_case() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("Options").join("Red")).unwrap();
        std::fs::write(base.path().join("Options").join("Red").join("icon.png"), b"").unwrap();

        for written in ["Options\\Red\\icon.png", "options/red/ICON.png", "./Options/Red/icon.png", "OPTIONS\\red/Icon.PNG"] {
            let fixed = fix_path_casing(base.path(), Path::new(written)).await.unwrap();
            assert_components(&fixed, &["Options", "Red", "icon.png"]);
            assert!(base.path().join(&fixed).is_file(), "{written} -> {fixed:?}");
        }
    }

    #[tokio::test]
    async fn fix_path_casing_never_fails_on_missing_paths() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("Options")).unwrap();

        let fixed = fix_path_casing(base.path(), Path::new("options/Missing/deeper/x.png")).await.unwrap();
        assert_components(&fixed, &["Options", "Missing", "deeper", "x.png"]);
    }

    #[tokio::test]
    async fn fix_path_casing_never_escapes_base() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("etc")).unwrap();

        for (written, expected) in [
            ("..\\..\\etc", &["etc"][..]),
            ("../../etc", &["etc"][..]),
            ("/etc", &["etc"][..]),
            ("C:\\Windows\\etc", &["Windows", "etc"][..]),
            ("C:etc", &["etc"][..]),
            ("\\\\server\\share\\etc", &["server", "share", "etc"][..]),
            ("\\\\?\\C:\\etc", &["?", "etc"][..]),
            ("\\\\.\\C:\\etc", &["etc"][..]),
        ] {
            let fixed = fix_path_casing(base.path(), Path::new(written)).await.unwrap();
            assert!(fixed.is_relative(), "{written} -> {fixed:?}");
            assert!(
                fixed.components().all(|c| matches!(c, std::path::Component::Normal(_))),
                "{written} -> {fixed:?}"
            );
            assert!(base.path().join(&fixed).starts_with(base.path()), "{written} -> {fixed:?}");
            assert_components(&fixed, expected);
        }
    }

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

    /// (files, total bytes) under `dir`, not following symlinks.
    fn tree_size(dir: &Path) -> (usize, u64) {
        let mut out = (0, 0);
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                let (n, b) = tree_size(&entry.path());
                out = (out.0 + n, out.1 + b);
            } else if ft.is_file() {
                out = (out.0 + 1, out.1 + entry.metadata().unwrap().len());
            }
        }
        out
    }

    /// A folder with two mods in it, like a user's existing mod folder.
    fn mod_collection(root: &Path) {
        for name in ["ModA", "ModB"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join(patch_file_name(0)), vec![7u8; 1000]).unwrap();
        }
    }

    /// The bug report: copying a folder into a folder inside itself used to
    /// re-read the growing copy and nest `mods/mods/mods/...` (each level
    /// holding another full copy) until the path got too long -- 810 levels
    /// deep on Linux for a 2 KB folder.
    #[tokio::test]
    async fn copy_dir_recursive_refuses_destination_inside_source() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("mods");
        mod_collection(&src);
        let before = tree_size(&src);

        let err = copy_dir_recursive(&src, &src.join("mods"), &[]).await.unwrap_err();

        assert!(format!("{err:#}").contains("into itself"), "{err:#}");
        assert!(!src.join("mods").exists(), "nothing may be created");
        assert_eq!(tree_size(&src), before);
    }

    #[tokio::test]
    async fn copy_dir_recursive_refuses_source_inside_destination() {
        let root = tempfile::tempdir().unwrap();
        mod_collection(root.path());
        let before = tree_size(root.path());

        assert!(copy_dir_recursive(&root.path().join("ModA"), root.path(), &[]).await.is_err());
        assert_eq!(tree_size(root.path()), before);
    }

    #[tokio::test]
    async fn copy_dir_recursive_refuses_copying_onto_itself() {
        let root = tempfile::tempdir().unwrap();
        mod_collection(root.path());
        let before = tree_size(root.path());

        assert!(copy_dir_recursive(root.path(), root.path(), &[]).await.is_err());
        // Copying a file onto itself would truncate it; nothing changed.
        assert_eq!(tree_size(root.path()), before);
    }

    #[tokio::test]
    async fn copy_dir_recursive_refuses_overlap_spelled_with_dotdot() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("mods");
        mod_collection(&src);
        let dst = src.join("ModA").join("..").join("copy");

        assert!(copy_dir_recursive(&src, &dst, &[]).await.is_err());
        assert!(!src.join("copy").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_dir_recursive_refuses_overlap_through_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("mods");
        mod_collection(&src);
        let link = root.path().join("link-to-mods");
        std::os::unix::fs::symlink(&src, &link).unwrap();

        assert!(copy_dir_recursive(&src, &link.join("copy"), &[]).await.is_err());
        assert!(copy_dir_recursive(&link, &src.join("copy"), &[]).await.is_err());
        assert!(!src.join("copy").exists());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[tokio::test]
    async fn copy_dir_recursive_refuses_overlap_spelled_in_other_case() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("mods");
        mod_collection(&src);

        assert!(copy_dir_recursive(&src, &root.path().join("MODS").join("copy"), &[]).await.is_err());
        assert!(!src.join("copy").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_dir_recursive_does_not_follow_a_symlink_loop() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        mod_collection(src.path());
        std::os::unix::fs::symlink(src.path(), src.path().join("ModA").join("loop")).unwrap();

        copy_dir_recursive(src.path(), &dst.path().join("copy"), &[]).await.unwrap();

        assert_eq!(tree_size(&dst.path().join("copy")), tree_size(src.path()));
        assert!(!dst.path().join("copy/ModA/loop").exists());
    }

    #[tokio::test]
    async fn copy_dir_recursive_stops_at_the_depth_cap() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let mut deep = src.path().to_path_buf();
        for _ in 0..=MAX_COPY_DEPTH {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();

        let err = copy_dir_recursive(src.path(), &dst.path().join("copy"), &[]).await.unwrap_err();
        assert!(format!("{err:#}").contains("levels deep"), "{err:#}");

        // One level shallower is fine.
        std::fs::remove_dir(&deep).unwrap();
        copy_dir_recursive(src.path(), &dst.path().join("copy2"), &[]).await.unwrap();
    }

    fn patch_file_name(index: u32) -> String {
        format!("0123456789abcdef.patch_{}", index)
    }

    async fn touch(path: &Path) {
        tokio::fs::write(path, b"").await.unwrap();
    }

    #[test]
    fn is_patch_filename_matches_hd2_patch_files() {
        assert!(is_patch_filename("0123456789abcdef.patch_0"));
        assert!(is_patch_filename("0123456789abcdef.patch_12.gpu_resources"));
        assert!(is_patch_filename("0123456789abcdef.patch_12.stream"));
        assert!(!is_patch_filename("readme.txt"));
        assert!(!is_patch_filename("0123456789abcdef.patch_"));
    }

    #[test]
    fn natural_cmp_orders_numeric_runs_numerically() {
        let mut v = vec!["Option 10", "Option 2", "Option 1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["Option 1", "Option 2", "Option 10"]);
    }

    #[test]
    fn natural_cmp_is_case_insensitive_for_text() {
        assert_eq!(natural_cmp("blue", "Red"), std::cmp::Ordering::Less);
    }

    #[tokio::test]
    async fn detect_patch_layout_root_has_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join(patch_file_name(0))).await;

        let layout = detect_patch_layout(dir.path()).await.unwrap();
        assert_eq!(layout, PatchLayout::RootHasFiles);
    }

    #[tokio::test]
    async fn detect_patch_layout_single_wrapper_folder() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("ModName");
        tokio::fs::create_dir(&wrapper).await.unwrap();
        touch(&wrapper.join(patch_file_name(0))).await;

        let layout = detect_patch_layout(dir.path()).await.unwrap();
        assert_eq!(layout, PatchLayout::Options(vec!["ModName".to_string()]));
    }

    #[tokio::test]
    async fn detect_patch_layout_multiple_variant_folders_sorted_naturally() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Option 10", "Option 2", "Option 1"] {
            let d = dir.path().join(name);
            tokio::fs::create_dir(&d).await.unwrap();
            touch(&d.join(patch_file_name(0))).await;
        }

        let layout = detect_patch_layout(dir.path()).await.unwrap();
        assert_eq!(
            layout,
            PatchLayout::Options(vec![
                "Option 1".to_string(),
                "Option 2".to_string(),
                "Option 10".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn detect_patch_layout_nested_within_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("Variants").join("Red");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        touch(&nested.join(patch_file_name(0))).await;

        let layout = detect_patch_layout(dir.path()).await.unwrap();
        assert_eq!(
            layout,
            PatchLayout::Options(vec!["Variants/Red".to_string()])
        );
    }

    #[tokio::test]
    async fn detect_patch_layout_none_found() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("readme.txt")).await;

        let layout = detect_patch_layout(dir.path()).await.unwrap();
        assert_eq!(layout, PatchLayout::NoneFound);
    }

    #[tokio::test]
    async fn detect_patch_layout_does_not_recurse_into_qualifying_dir() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("ModName");
        let nested = wrapper.join("NestedIgnored");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        touch(&wrapper.join(patch_file_name(0))).await;
        touch(&nested.join(patch_file_name(1))).await;

        let layout = detect_patch_layout(dir.path()).await.unwrap();
        // Only the top-level wrapper is reported, not the nested dir inside it.
        assert_eq!(layout, PatchLayout::Options(vec!["ModName".to_string()]));
    }
}