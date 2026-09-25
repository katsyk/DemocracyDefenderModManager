//! Auto-detecting the Helldivers 2 (Steam app 553850) install directory, so
//! a new player can get from launch to Deploy without ever opening
//! Settings.
//!
//! Steam roots are platform-specific well-known locations; from each root,
//! `steamapps/libraryfolders.vdf` lists every library Steam knows about
//! (plus the root itself, which is always an implicit library). Each
//! library's `steamapps/appmanifest_553850.acf` (if the game is installed
//! there) names the game's install directory under
//! `steamapps/common/`.

use std::path::{Path, PathBuf};

/// Helldivers 2's Steam AppID.
pub const HELLDIVERS_2_APP_ID: &str = "553850";

/// Well-known Steam install locations to check, in order. Only existing
/// directories are returned.
pub fn find_steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = windows_steam_path_from_registry() {
            roots.push(path);
        }
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join(".steam/steam"));
            roots.push(home.join(".local/share/Steam"));
            roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
        }
    }

    let mut seen = std::collections::HashSet::new();
    roots
        .into_iter()
        .filter(|p| p.is_dir() && seen.insert(p.clone()))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_steam_path_from_registry() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey("Software\\Valve\\Steam").ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    Some(PathBuf::from(path))
}

/// Every library folder Steam knows about for a given root: the root
/// itself (always an implicit library) plus every `path` value found in
/// its `steamapps/libraryfolders.vdf`, deduplicated.
pub fn find_libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![steam_root.to_path_buf()];

    let vdf_path = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(content) = std::fs::read_to_string(&vdf_path) {
        for path in parse_library_paths(&content) {
            if !libraries.contains(&path) {
                libraries.push(path);
            }
        }
    }

    libraries
}

/// Extract every `"path"` value from a `libraryfolders.vdf`'s contents.
/// Deliberately tolerant: a line-based scan for `"path"  "<value>"` rather
/// than a full VDF/KeyValues parser, since that's all this file format
/// needs here.
pub fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix("\"path\"")?;
            extract_quoted_value(rest.trim_start()).map(PathBuf::from)
        })
        .collect()
}

/// Read the `installdir` value out of a `steamapps/appmanifest_<id>.acf`'s
/// contents, if present.
pub fn parse_installdir(acf: &str) -> Option<String> {
    acf.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("\"installdir\"")?;
        extract_quoted_value(rest.trim_start())
    })
}

/// Parse a single VDF/KeyValues quoted string starting at `rest` (which
/// must begin with `"`), unescaping `\"` and `\\` as it goes. `None` if
/// `rest` doesn't start with a quote or the string is never closed.
fn extract_quoted_value(rest: &str) -> Option<String> {
    let mut chars = rest.chars();
    if chars.next()? != '"' {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            value.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => return Some(value),
            _ => value.push(c),
        }
    }
    None
}

/// Read a library's `steamapps/appmanifest_<app_id>.acf` and return its
/// `installdir`, if the manifest exists and parses.
pub fn find_installdir(library: &Path, app_id: &str) -> Option<String> {
    let acf_path = library
        .join("steamapps")
        .join(format!("appmanifest_{app_id}.acf"));
    let content = std::fs::read_to_string(acf_path).ok()?;
    parse_installdir(&content)
}

/// The same validity checks `Settings::validate` applies to a configured
/// game path (existence, `tools/`, `data/`, `bin/`, `bin/helldivers2.exe`)
/// -- duplicated here (rather than shared) since `Settings::validate` also
/// checks a non-empty path and produces user-facing error messages, which
/// don't apply to a detection candidate.
async fn is_valid_game_path(path: &Path) -> bool {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return false;
    }
    if !tokio::fs::try_exists(path.join("tools")).await.unwrap_or(false) {
        return false;
    }
    if !tokio::fs::try_exists(path.join("data")).await.unwrap_or(false) {
        return false;
    }
    let bin = path.join("bin");
    if !tokio::fs::try_exists(&bin).await.unwrap_or(false) {
        return false;
    }
    tokio::fs::try_exists(bin.join("helldivers2.exe"))
        .await
        .unwrap_or(false)
}

/// Search every known Steam root's libraries for Helldivers 2, returning
/// the first install directory that actually passes the same checks
/// `Settings::validate` would apply. `None` if nothing valid is found
/// anywhere -- never an error, since "Steam isn't installed" or "the game
/// isn't installed" are both entirely normal outcomes.
pub async fn detect_game_path() -> Option<PathBuf> {
    for root in find_steam_roots() {
        for library in find_libraries(&root) {
            if let Some(installdir) = find_installdir(&library, HELLDIVERS_2_APP_ID) {
                let candidate = library.join("steamapps").join("common").join(installdir);
                if is_valid_game_path(&candidate).await {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Case-insensitive substring every known Helldivers 2 process name
/// contains: the native Windows exe, and the same exe's name as reported
/// under Linux/Proton (where `sysinfo` truncates process names to 15
/// characters -- this needle is only 11, so it always survives that).
const GAME_PROCESS_NAME_NEEDLE: &str = "helldivers2";

/// Pure decision logic over already-collected process names, so this is
/// testable without actually spawning a process or touching `sysinfo`.
fn process_names_indicate_game_running<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    names
        .map(|n| n.to_ascii_lowercase())
        .any(|n| n.contains(GAME_PROCESS_NAME_NEEDLE))
}

/// Cheap, best-effort check for whether Helldivers 2 is currently running --
/// used to refuse a browser-triggered `afterInstall: deploy` while it is
/// (overwriting a running game's data files is unsafe). Best-effort: a
/// false negative just means a deploy is attempted anyway (and can still
/// fail safely on its own); this never blocks the game from launching or
/// anything other than that one bridge step.
pub fn is_game_running() -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    process_names_indicate_game_running(system.processes().values().filter_map(|p| p.name().to_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_process_detected_by_windows_style_name() {
        assert!(process_names_indicate_game_running(["explorer.exe", "HellDivers2.exe"].into_iter()));
    }

    #[test]
    fn game_process_detected_case_insensitively() {
        assert!(process_names_indicate_game_running(["helldivers2.exe"].into_iter()));
    }

    #[test]
    fn game_process_detected_when_truncated_to_15_chars_like_linux() {
        // "HellDivers2.exe" truncated to sysinfo's 15-char Linux limit.
        assert!(process_names_indicate_game_running(["HellDivers2.ex"].into_iter()));
    }

    #[test]
    fn unrelated_processes_do_not_match() {
        assert!(!process_names_indicate_game_running(["steam.exe", "explorer.exe", "bash"].into_iter()));
    }

    #[test]
    fn empty_process_list_does_not_match() {
        assert!(!process_names_indicate_game_running(std::iter::empty()));
    }

    #[test]
    fn parses_multiple_library_paths() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"label"		""
	}
}
"#;
        let paths = parse_library_paths(vdf);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("C:\\Program Files (x86)\\Steam"),
                PathBuf::from("D:\\SteamLibrary"),
            ]
        );
    }

    #[test]
    fn parses_linux_style_unescaped_paths() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/user/.steam/steam"
	}
}
"#;
        let paths = parse_library_paths(vdf);
        assert_eq!(paths, vec![PathBuf::from("/home/user/.steam/steam")]);
    }

    #[test]
    fn parses_installdir_from_acf() {
        let acf = r#"
"AppState"
{
	"appid"		"553850"
	"Universe"		"1"
	"name"		"Helldivers 2"
	"installdir"		"Helldivers 2"
	"StateFlags"		"4"
}
"#;
        assert_eq!(parse_installdir(acf).as_deref(), Some("Helldivers 2"));
    }

    #[test]
    fn missing_installdir_is_none() {
        let acf = r#"
"AppState"
{
	"appid"		"553850"
}
"#;
        assert!(parse_installdir(acf).is_none());
    }

    #[test]
    fn malformed_vdf_yields_no_paths_not_an_error() {
        let paths = parse_library_paths("not valid vdf at all { [ \" ");
        assert!(paths.is_empty());
    }

    fn make_valid_game_dir(path: &Path) {
        std::fs::create_dir_all(path.join("tools")).unwrap();
        std::fs::create_dir_all(path.join("data")).unwrap();
        std::fs::create_dir_all(path.join("bin")).unwrap();
        std::fs::write(path.join("bin").join("helldivers2.exe"), b"").unwrap();
    }

    #[tokio::test]
    async fn detects_game_in_a_non_default_library() {
        let steam_root = tempfile::tempdir().unwrap();
        let other_library = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(steam_root.path().join("steamapps")).unwrap();
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            other_library.path().display().to_string().replace('\\', "\\\\")
        );
        std::fs::write(
            steam_root.path().join("steamapps/libraryfolders.vdf"),
            vdf,
        )
        .unwrap();

        // Not installed in the default library.
        let common_default = steam_root.path().join("steamapps/common/Helldivers 2");
        let _ = common_default; // intentionally absent

        // Installed in the second library.
        std::fs::create_dir_all(other_library.path().join("steamapps")).unwrap();
        std::fs::write(
            other_library
                .path()
                .join("steamapps/appmanifest_553850.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"Helldivers 2\"\n}\n",
        )
        .unwrap();
        let game_dir = other_library.path().join("steamapps/common/Helldivers 2");
        make_valid_game_dir(&game_dir);

        let libraries = find_libraries(steam_root.path());
        assert_eq!(libraries.len(), 2);

        let mut found = None;
        for library in &libraries {
            if let Some(installdir) = find_installdir(library, HELLDIVERS_2_APP_ID) {
                let candidate = library.join("steamapps/common").join(installdir);
                if is_valid_game_path(&candidate).await {
                    found = Some(candidate);
                    break;
                }
            }
        }

        assert_eq!(found, Some(game_dir));
    }

    #[tokio::test]
    async fn missing_manifest_in_a_library_is_skipped_cleanly() {
        let library = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(library.path().join("steamapps")).unwrap();
        // No appmanifest_553850.acf at all.

        assert!(find_installdir(library.path(), HELLDIVERS_2_APP_ID).is_none());
    }

    #[tokio::test]
    async fn candidate_missing_required_subdirs_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // Only create the directory itself, none of tools/data/bin.
        assert!(!is_valid_game_path(dir.path()).await);
    }

    #[tokio::test]
    async fn candidate_with_all_required_pieces_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        make_valid_game_dir(dir.path());
        assert!(is_valid_game_path(dir.path()).await);
    }
}
