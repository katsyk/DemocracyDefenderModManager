//! The single place that decides whether a directory is a Helldivers 2
//! install, and says *why* when it isn't.
//!
//! Used by `Settings::validate` (deploy/purge/check_settings), by Steam
//! auto-detection (`steam::detect_game_path`), and by the Settings page
//! itself through the `validate_game_path` command.
//!
//! The Settings page used to do this check itself with the fs plugin's
//! `exists`, which goes through Tauri's fs scope. On Linux that scope's
//! `**` glob never matches a path with a dot-directory in it (Tauri sets
//! `require_literal_leading_dot` on Unix), and the default Steam library
//! lives under `~/.local/share/Steam` (or `~/.steam/steam`, or Flatpak's
//! `~/.var/app/...`) -- so every default Linux install was reported as
//! "Game path is invalid!" even though the backend accepted it. Doing the
//! check here, with plain `std::fs`, removes that split entirely.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Directory Steam installs the game into under `steamapps/common/`.
pub const GAME_INSTALL_DIR: &str = "Helldivers 2";
/// The game executable inside `bin/`.
pub const GAME_EXE: &str = "helldivers2.exe";

/// Why a path isn't a usable Helldivers 2 install. `code()` is stable and
/// is what the frontend keys its translated message off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamePathProblem {
    Empty,
    NotFound,
    NotADirectory,
    /// `/run/user/<uid>/doc/...`: an xdg-desktop-portal document-portal
    /// path, which only exposes what was explicitly shared, not the real
    /// game directory.
    PortalPath,
    /// The OS refused to let us look (permissions, a dead network/NTFS
    /// mount, ...). Carries the OS error text.
    Unreadable(String),
    MissingTools,
    MissingData,
    MissingBin,
    MissingExe,
}

impl GamePathProblem {
    pub fn code(&self) -> &'static str {
        match self {
            GamePathProblem::Empty => "empty",
            GamePathProblem::NotFound => "not_found",
            GamePathProblem::NotADirectory => "not_a_directory",
            GamePathProblem::PortalPath => "portal_path",
            GamePathProblem::Unreadable(_) => "unreadable",
            GamePathProblem::MissingTools => "missing_tools",
            GamePathProblem::MissingData => "missing_data",
            GamePathProblem::MissingBin => "missing_bin",
            GamePathProblem::MissingExe => "missing_exe",
        }
    }

    /// English, log/error-message form. The UI shows a translated message
    /// keyed by `code()` instead, plus `detail()` where there is one.
    pub fn describe(&self, path: &Path) -> String {
        match self {
            GamePathProblem::Empty => "game path is empty".to_string(),
            GamePathProblem::NotFound => format!("game path {:?} does not exist", path),
            GamePathProblem::NotADirectory => format!("game path {:?} is not a folder", path),
            GamePathProblem::PortalPath => format!(
                "game path {:?} is a desktop-portal document path, not the real game folder",
                path
            ),
            GamePathProblem::Unreadable(e) => format!("can't read game path {:?}: {}", path, e),
            GamePathProblem::MissingTools => format!("{:?} has no \"tools\" folder", path),
            GamePathProblem::MissingData => format!("{:?} has no \"data\" folder", path),
            GamePathProblem::MissingBin => format!("{:?} has no \"bin\" folder", path),
            GamePathProblem::MissingExe => {
                format!("{:?} has no \"bin/{}\"", path, GAME_EXE)
            }
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            GamePathProblem::Unreadable(e) => Some(e.clone()),
            _ => None,
        }
    }
}

/// Result of checking a user-supplied path, as sent to the Settings page.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GamePathReport {
    pub valid: bool,
    /// The actual game root, when `valid`. May differ from the input when
    /// the user picked a folder above or below it (see [`resolve`]).
    pub resolved_path: Option<String>,
    /// [`GamePathProblem::code`] when not `valid`.
    pub code: Option<String>,
    /// Extra detail (e.g. an OS error) for `code`, if any.
    pub detail: Option<String>,
    /// Full English description, for logs.
    pub message: Option<String>,
}

impl GamePathReport {
    pub fn from_result(input: &Path, result: &Result<PathBuf, GamePathProblem>) -> Self {
        match result {
            Ok(root) => GamePathReport {
                valid: true,
                resolved_path: Some(root.to_string_lossy().into_owned()),
                code: None,
                detail: None,
                message: None,
            },
            Err(problem) => GamePathReport {
                valid: false,
                resolved_path: None,
                code: Some(problem.code().to_string()),
                detail: problem.detail(),
                message: Some(problem.describe(input)),
            },
        }
    }
}

/// Resolve `input` to a Helldivers 2 game root.
///
/// Accepts the root itself, and also the levels people commonly pick by
/// mistake, normalising to the real root:
/// - a folder *inside* the root (`data/`, `bin/`, `tools/`), or
///   `bin/helldivers2.exe` itself;
/// - a folder *above* it: `steamapps/common`, `steamapps`, or the Steam
///   library folder.
///
/// When nothing matches, the problem reported is the one for `input`
/// itself, so the message talks about what the user actually typed.
pub async fn resolve(input: &Path) -> Result<PathBuf, GamePathProblem> {
    let input = input.to_path_buf();
    tokio::task::spawn_blocking(move || resolve_blocking(&input))
        .await
        .unwrap_or_else(|e| Err(GamePathProblem::Unreadable(e.to_string())))
}

pub fn resolve_blocking(input: &Path) -> Result<PathBuf, GamePathProblem> {
    if input.as_os_str().is_empty() {
        return Err(GamePathProblem::Empty);
    }

    let input = strip_trailing_separators(input);

    match std::fs::metadata(&input) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            // `bin/helldivers2.exe` picked directly.
            if file_name_eq(&input, GAME_EXE) {
                if let Some(root) = input.parent().and_then(Path::parent) {
                    if check_root(root).is_ok() {
                        return Ok(root.to_path_buf());
                    }
                }
            }
            return Err(GamePathProblem::NotADirectory);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(if is_portal_document_path(&input) {
                GamePathProblem::PortalPath
            } else {
                GamePathProblem::NotFound
            });
        }
        Err(e) => return Err(GamePathProblem::Unreadable(e.to_string())),
    }

    let direct = check_root(&input);
    if direct.is_ok() {
        return Ok(input);
    }

    for candidate in candidates_around(&input) {
        if check_root(&candidate).is_ok() {
            return Ok(candidate);
        }
    }

    if is_portal_document_path(&input) {
        return Err(GamePathProblem::PortalPath);
    }
    direct.map(|_| input)
}

/// Other directories to try when `dir` itself isn't the game root.
fn candidates_around(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();

    // Picked a folder inside the root.
    if ["data", "bin", "tools"].iter().any(|n| file_name_eq(dir, n)) {
        if let Some(parent) = dir.parent() {
            out.push(parent.to_path_buf());
        }
    }

    // Picked a folder above the root: `common/`, `steamapps/`, or the
    // library folder itself.
    for suffix in [
        &[GAME_INSTALL_DIR][..],
        &["common", GAME_INSTALL_DIR][..],
        &["steamapps", "common", GAME_INSTALL_DIR][..],
    ] {
        if let Some(p) = join_case_insensitive(dir, suffix) {
            out.push(p);
        }
    }

    out
}

/// Check that `root` has `tools/`, `data/`, `bin/` and
/// `bin/helldivers2.exe`. The folders must match exactly (deploy writes to
/// `data/` by that exact name); the exe is found case-insensitively since
/// nothing depends on its exact spelling.
pub fn check_root(root: &Path) -> Result<(), GamePathProblem> {
    for (name, problem) in [
        ("tools", GamePathProblem::MissingTools),
        ("data", GamePathProblem::MissingData),
        ("bin", GamePathProblem::MissingBin),
    ] {
        match std::fs::metadata(root.join(name)) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(problem),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(problem),
            Err(e) => return Err(GamePathProblem::Unreadable(e.to_string())),
        }
    }

    let bin = root.join("bin");
    if bin.join(GAME_EXE).is_file() {
        return Ok(());
    }
    match std::fs::read_dir(&bin) {
        Ok(entries) => {
            let found = entries.flatten().any(|e| {
                e.file_name().to_string_lossy().eq_ignore_ascii_case(GAME_EXE)
                    && e.file_type().map(|t| t.is_file()).unwrap_or(false)
            });
            if found { Ok(()) } else { Err(GamePathProblem::MissingExe) }
        }
        Err(e) => Err(GamePathProblem::Unreadable(e.to_string())),
    }
}

fn file_name_eq(path: &Path, name: &str) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        .unwrap_or(false)
}

/// Follow `parts` below `base`, matching each component case-insensitively
/// (so `SteamApps` from very old Steam installs, or a library on a
/// case-sensitive drive with odd casing, still work). `None` if any
/// component is missing.
fn join_case_insensitive(base: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut current = base.to_path_buf();
    for part in parts {
        let exact = current.join(part);
        if exact.is_dir() {
            current = exact;
            continue;
        }
        let entry = std::fs::read_dir(&current).ok()?.flatten().find(|e| {
            e.file_name().to_string_lossy().eq_ignore_ascii_case(part)
                && e.path().is_dir()
        })?;
        current = entry.path();
    }
    Some(current)
}

fn strip_trailing_separators(path: &Path) -> PathBuf {
    // `Path::components` already drops a trailing `/`; rebuilding also
    // collapses `a//b` and `a/./b`.
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        out
    }
}

/// `/run/user/<uid>/doc/...` -- where the xdg-desktop-portal document
/// portal exposes files it shares with sandboxed apps.
/// Linux-only: always `false` elsewhere, so no Windows/macOS path is ever
/// classified as one.
pub fn is_portal_document_path(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let s = path.to_string_lossy();
        s.starts_with("/run/user/") && s.split('/').nth(4) == Some("doc")
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_game(root: &Path) {
        std::fs::create_dir_all(root.join("tools")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join(GAME_EXE), b"").unwrap();
    }

    fn library_with_game() -> (tempfile::TempDir, PathBuf) {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path().join("steamapps/common").join(GAME_INSTALL_DIR);
        make_game(&root);
        (lib, root)
    }

    #[test]
    fn accepts_the_root() {
        let (_lib, root) = library_with_game();
        assert_eq!(resolve_blocking(&root), Ok(root.clone()));
    }

    #[test]
    fn accepts_root_with_trailing_slash() {
        let (_lib, root) = library_with_game();
        let with_slash = PathBuf::from(format!("{}/", root.display()));
        assert_eq!(resolve_blocking(&with_slash), Ok(root));
    }

    #[test]
    fn walks_up_from_data_bin_and_tools() {
        let (_lib, root) = library_with_game();
        for sub in ["data", "bin", "tools"] {
            assert_eq!(resolve_blocking(&root.join(sub)), Ok(root.clone()), "{sub}");
        }
    }

    #[test]
    fn walks_up_from_the_exe_itself() {
        let (_lib, root) = library_with_game();
        assert_eq!(resolve_blocking(&root.join("bin").join(GAME_EXE)), Ok(root));
    }

    #[test]
    fn walks_down_from_common_steamapps_and_library() {
        let (lib, root) = library_with_game();
        assert_eq!(resolve_blocking(&lib.path().join("steamapps/common")), Ok(root.clone()));
        assert_eq!(resolve_blocking(&lib.path().join("steamapps")), Ok(root.clone()));
        assert_eq!(resolve_blocking(lib.path()), Ok(root));
    }

    #[test]
    fn accepts_paths_under_dot_directories() {
        // The default Linux Steam library is ~/.local/share/Steam; the old
        // frontend check rejected anything under a dot-directory.
        let home = tempfile::tempdir().unwrap();
        let root = home
            .path()
            .join(".local/share/Steam/steamapps/common")
            .join(GAME_INSTALL_DIR);
        make_game(&root);
        assert_eq!(resolve_blocking(&root), Ok(root));
    }

    #[test]
    fn accepts_spaces_and_unicode() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("Jeux Vidéo ☕/Steam Library/steamapps/common").join(GAME_INSTALL_DIR);
        make_game(&root);
        assert_eq!(resolve_blocking(&root), Ok(root));
    }

    #[cfg(unix)]
    #[test]
    fn accepts_a_symlinked_library() {
        let real = tempfile::tempdir().unwrap();
        let root = real.path().join("steamapps/common").join(GAME_INSTALL_DIR);
        make_game(&root);
        let links = tempfile::tempdir().unwrap();
        let link = links.path().join("SteamLibrary");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        let via_link = link.join("steamapps/common").join(GAME_INSTALL_DIR);
        assert_eq!(resolve_blocking(&via_link), Ok(via_link.clone()));
        assert_eq!(resolve_blocking(&link), Ok(via_link));
    }

    #[test]
    fn exe_is_found_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tools")).unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/HellDivers2.EXE"), b"").unwrap();
        assert_eq!(resolve_blocking(dir.path()), Ok(dir.path().to_path_buf()));
    }

    #[test]
    fn reports_specific_problems() {
        assert_eq!(resolve_blocking(Path::new("")), Err(GamePathProblem::Empty));

        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_blocking(&dir.path().join("nope")),
            Err(GamePathProblem::NotFound)
        );

        std::fs::write(dir.path().join("file"), b"").unwrap();
        assert_eq!(
            resolve_blocking(&dir.path().join("file")),
            Err(GamePathProblem::NotADirectory)
        );

        let root = dir.path().join("game");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(resolve_blocking(&root), Err(GamePathProblem::MissingTools));
        std::fs::create_dir_all(root.join("tools")).unwrap();
        assert_eq!(resolve_blocking(&root), Err(GamePathProblem::MissingData));
        std::fs::create_dir_all(root.join("data")).unwrap();
        assert_eq!(resolve_blocking(&root), Err(GamePathProblem::MissingBin));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        assert_eq!(resolve_blocking(&root), Err(GamePathProblem::MissingExe));
        std::fs::write(root.join("bin").join(GAME_EXE), b"").unwrap();
        assert_eq!(resolve_blocking(&root), Ok(root));
    }

    #[test]
    fn a_data_folder_that_isnt_in_a_game_reports_the_input_problem() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        assert_eq!(resolve_blocking(&data), Err(GamePathProblem::MissingTools));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detects_portal_document_paths() {
        assert!(is_portal_document_path(Path::new("/run/user/1000/doc/abcd1234/Helldivers 2")));
        assert!(!is_portal_document_path(Path::new("/run/user/1000/gvfs/x")));
        assert!(!is_portal_document_path(Path::new("/home/u/.local/share/Steam")));
        assert_eq!(
            resolve_blocking(Path::new("/run/user/1000/doc/deadbeef/Helldivers 2")),
            Err(GamePathProblem::PortalPath)
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn portal_paths_are_never_detected_off_linux() {
        assert!(!is_portal_document_path(Path::new("/run/user/1000/doc/abcd1234/Helldivers 2")));
        assert!(!is_portal_document_path(Path::new(r"C:\run\user\1000\doc\x")));
        assert_eq!(
            resolve_blocking(Path::new("/run/user/1000/doc/deadbeef/Helldivers 2")),
            Err(GamePathProblem::NotFound)
        );
    }

    #[test]
    fn report_carries_code_and_message() {
        let p = Path::new("/nonexistent/for/sure");
        let r = GamePathReport::from_result(p, &Err(GamePathProblem::NotFound));
        assert!(!r.valid);
        assert_eq!(r.code.as_deref(), Some("not_found"));
        assert!(r.message.unwrap().contains("/nonexistent/for/sure"));
    }
}
