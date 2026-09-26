//! Where DDMM keeps its data: `mods/`, `settings.json`, `profiles.json`, and
//! logs.
//!
//! Earlier previews always used the executable's own directory. That
//! breaks under a real installer: an NSIS per-machine install lands in
//! `Program Files` (not writable by a normal user), an AppImage runs from a
//! read-only squashfs mount, and a `.deb` puts the binary in `/usr/bin`.
//! This module decides, once at startup, whether to keep the old
//! next-to-the-executable ("portable") behavior or switch to the OS's
//! per-user application data directory.

use std::path::{Path, PathBuf};

/// The file that marks a directory as an intentional portable install. Only
/// this name is recognized (not e.g. `.portable` too) so there's exactly
/// one documented way to opt in -- see `docs/getting-started/download.md`.
pub const PORTABLE_MARKER_FILENAME: &str = "portable.txt";

/// This app's identifier, matching `tauri.conf.json`. Used to compute the
/// per-user app data directory independently of a running `AppHandle` --
/// see [`platform_app_data_dir`] for why.
pub const APP_IDENTIFIER: &str = "io.github.katsyk.ddmm";

/// Which way [`decide_base_dir`] went, and why -- both logged at startup
/// and shown (read-only) in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseDirKind {
    Portable,
    AppData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseDirDecision {
    pub path: PathBuf,
    pub kind: BaseDirKind,
    pub reason: &'static str,
}

/// Decide where DDMM should keep its data.
///
/// Portable mode wins when the executable's directory looks like a
/// portable install -- a `portable.txt` marker, or (for existing
/// upstream-style installs that predate this marker) a `mods/` directory or
/// `settings.json` file already sitting there -- **and** that directory is
/// actually writable. Otherwise (including when it looks portable but
/// isn't writable, e.g. `Program Files`) `app_data_dir` is used.
///
/// `app_data_dir` is a parameter rather than computed here so this stays a
/// pure, easily testable decision; see [`platform_app_data_dir`] for what
/// callers should normally pass.
pub fn decide_base_dir(exe_dir: &Path, app_data_dir: &Path) -> BaseDirDecision {
    if let Some(reason) = portable_reason(exe_dir) {
        if is_dir_writable(exe_dir) {
            return BaseDirDecision {
                path: exe_dir.to_path_buf(),
                kind: BaseDirKind::Portable,
                reason,
            };
        }
        // Looks portable but we can't write there (e.g. Program Files under
        // a per-machine NSIS install) -- fall through to app data.
    }

    BaseDirDecision {
        path: app_data_dir.to_path_buf(),
        kind: BaseDirKind::AppData,
        reason: "no writable portable marker or legacy data next to the executable",
    }
}

fn portable_reason(exe_dir: &Path) -> Option<&'static str> {
    if exe_dir.join(PORTABLE_MARKER_FILENAME).is_file() {
        return Some("portable.txt marker present");
    }
    if exe_dir.join("mods").is_dir() {
        return Some("legacy mods/ directory present next to the executable");
    }
    if exe_dir.join("settings.json").is_file() {
        return Some("legacy settings.json present next to the executable");
    }
    None
}

/// Best-effort writability probe: try to create (and immediately remove) a
/// throwaway file. There's no portable, race-free "can I write here" check
/// in std, so this is it.
pub(crate) fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".ddmm-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The file that records a user-chosen data folder ("Change" next to the
/// data folder in Settings). It lives outside the data folder it points at,
/// so it survives the move and is read before any data is: next to the
/// executable for a portable copy (so the portable folder stays
/// self-describing), and in [`installed_pointer_dir`] otherwise.
pub const POINTER_FILENAME: &str = "ddmm-data-location.json";

/// Contents of [`POINTER_FILENAME`]. A relative `path` is relative to the
/// folder the pointer file is in: a portable copy whose data folder is
/// inside the portable folder keeps working when the whole portable folder
/// is moved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PointerFile {
    pub version: u32,
    pub path: PathBuf,
}

pub const POINTER_VERSION: u32 = 1;

/// Where an installed (non-portable) DDMM keeps its pointer file: the OS
/// per-user config directory plus [`APP_IDENTIFIER`] (`%APPDATA%` on
/// Windows, `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or
/// `~/.config` on Linux).
pub fn installed_pointer_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_IDENTIFIER))
}

/// Why the chosen data folder can't be used this session. DDMM then starts
/// in a recovery screen instead of silently creating an empty folder (and
/// appearing to have lost every mod, e.g. with a USB drive unplugged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataDirProblem {
    /// The pointer names a folder that doesn't exist (or isn't a folder).
    Missing,
    /// The pointer file exists but can't be read or parsed.
    BadPointer(String),
}

/// The full data-folder decision: [`decide_base_dir`] plus any pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirDecision {
    /// The data folder to use this session.
    pub path: PathBuf,
    /// Portable (pointer next to the exe) or installed (pointer in the
    /// config dir). Decides where a new pointer is written.
    pub kind: BaseDirKind,
    pub reason: String,
    /// Where the data lives without a pointer ("Reset to default").
    pub default_path: PathBuf,
    /// Where this mode's pointer file is (or would be written).
    pub pointer_file: PathBuf,
    /// `path` came from a pointer file.
    pub pointed: bool,
    pub problem: Option<DataDirProblem>,
}

/// Read a pointer file. `Ok(None)` when there is none.
pub fn read_pointer(pointer_file: &Path) -> Result<Option<PathBuf>, String> {
    let data = match std::fs::read(pointer_file) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("couldn't read {}: {e}", pointer_file.display())),
    };
    let parsed: PointerFile = serde_json::from_slice(&data)
        .map_err(|e| format!("{} isn't a valid data-location file: {e}", pointer_file.display()))?;
    if parsed.path.as_os_str().is_empty() {
        return Err(format!("{} doesn't name a folder", pointer_file.display()));
    }
    let dir = pointer_file.parent().unwrap_or(Path::new("."));
    Ok(Some(if parsed.path.is_absolute() { parsed.path } else { dir.join(parsed.path) }))
}

/// Write a pointer file atomically (temp file, then rename over the old
/// one), creating its folder if needed. A `target` inside the pointer's own
/// folder is stored relative to it (see [`PointerFile`]).
pub fn write_pointer(pointer_file: &Path, target: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let dir = pointer_file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    let stored = relative_if_inside(target, dir).unwrap_or_else(|| target.to_path_buf());
    let data = serde_json::to_vec_pretty(&PointerFile { version: POINTER_VERSION, path: stored })?;
    let tmp = pointer_file.with_extension("json.tmp");
    std::fs::write(&tmp, data).with_context(|| format!("couldn't write {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, pointer_file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("couldn't write {}", pointer_file.display()));
    }
    Ok(())
}

/// Remove a pointer file ("Reset to default"). Already gone is fine.
pub fn remove_pointer(pointer_file: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(pointer_file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("couldn't remove {}: {e}", pointer_file.display())),
    }
}

fn relative_if_inside(target: &Path, dir: &Path) -> Option<PathBuf> {
    use crate::fs_util::{path_overlap, resolve_path, PathOverlap};
    match path_overlap(target, dir).ok()? {
        Some(PathOverlap::FirstInsideSecond) => {
            let (t, d) = (resolve_path(target).ok()?, resolve_path(dir).ok()?);
            t.strip_prefix(&d).ok().map(Path::to_path_buf)
        }
        _ => None,
    }
}

/// Decide the data folder, pointer files included. Precedence, highest
/// first (documented in `docs/using/data-folder.md`):
///
/// 1. a pointer file next to the executable (a portable copy whose data was
///    moved; the executable's folder doesn't need to be writable any more);
/// 2. a portable copy's own folder (see [`decide_base_dir`]);
/// 3. a pointer file in the installed pointer folder (an installed copy
///    whose data was moved);
/// 4. the per-user app data folder.
///
/// So a pointer always beats the default of its own mode. A pointed folder
/// that is missing, or a pointer that can't be read, is reported as a
/// [`DataDirProblem`], never replaced with a new empty folder.
pub fn decide_data_dir(exe_dir: &Path, app_data_dir: &Path, installed_pointer_dir: &Path) -> DataDirDecision {
    let portable_pointer = exe_dir.join(POINTER_FILENAME);
    let installed_pointer = installed_pointer_dir.join(POINTER_FILENAME);

    let pointed = |pointer_file: PathBuf, kind: BaseDirKind, default_path: &Path| -> Option<DataDirDecision> {
        match read_pointer(&pointer_file) {
            Ok(None) => None,
            Ok(Some(path)) => {
                let problem = (!path.is_dir()).then_some(DataDirProblem::Missing);
                Some(DataDirDecision {
                    path,
                    kind,
                    reason: format!("chosen in Settings ({})", pointer_file.display()),
                    default_path: default_path.to_path_buf(),
                    pointer_file,
                    pointed: true,
                    problem,
                })
            }
            Err(message) => Some(DataDirDecision {
                path: default_path.to_path_buf(),
                kind,
                reason: "unreadable data-location file".to_string(),
                default_path: default_path.to_path_buf(),
                pointer_file,
                pointed: true,
                problem: Some(DataDirProblem::BadPointer(message)),
            }),
        }
    };

    if let Some(d) = pointed(portable_pointer.clone(), BaseDirKind::Portable, exe_dir) {
        return d;
    }

    let base = decide_base_dir(exe_dir, app_data_dir);
    if base.kind == BaseDirKind::Portable {
        return DataDirDecision {
            path: base.path,
            kind: base.kind,
            reason: base.reason.to_string(),
            default_path: exe_dir.to_path_buf(),
            pointer_file: portable_pointer,
            pointed: false,
            problem: None,
        };
    }

    if let Some(d) = pointed(installed_pointer.clone(), BaseDirKind::AppData, app_data_dir) {
        return d;
    }

    DataDirDecision {
        path: base.path,
        kind: base.kind,
        reason: base.reason.to_string(),
        default_path: app_data_dir.to_path_buf(),
        pointer_file: installed_pointer,
        pointed: false,
        problem: None,
    }
}

/// [`decide_data_dir`] for this machine (the real executable, app data and
/// config folders). Used by the app at startup and by the browser's native
/// messaging host, so both always agree on where `bridge.json` is.
pub fn decide_data_dir_for_this_machine() -> DataDirDecision {
    let exe_dir = resolve_exe_dir();
    let app_data_dir = platform_app_data_dir().unwrap_or_else(|e| {
        eprintln!("warning: {e}; falling back to the executable directory for app data");
        exe_dir.clone()
    });
    let pointer_dir = installed_pointer_dir().unwrap_or_else(|| app_data_dir.clone());
    decide_data_dir(&exe_dir, &app_data_dir, &pointer_dir)
}

/// The directory containing the running executable.
///
/// For a Linux AppImage this is the directory of the `.AppImage` file
/// itself (`$APPIMAGE`, which the AppImage runtime always sets), not the
/// directory `std::env::current_exe()` would report -- that's a read-only
/// squashfs mount under `/tmp` that's different on every run, which would
/// never look portable and would defeat writability entirely.
pub fn resolve_exe_dir() -> PathBuf {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        let path = PathBuf::from(appimage);
        if let Some(parent) = path.parent() {
            return parent.to_path_buf();
        }
    }

    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The OS-appropriate per-user application data directory for
/// [`APP_IDENTIFIER`]: `%APPDATA%\<identifier>` on Windows,
/// `~/Library/Application Support/<identifier>` on macOS, and
/// `$XDG_DATA_HOME/<identifier>` (or `~/.local/share/<identifier>`) on
/// Linux.
///
/// This matches what `app.path().app_data_dir()` resolves to, but is
/// computed independently of a running `AppHandle`: the file-logging
/// target has to be configured on the `tauri_plugin_log` builder *before*
/// the app (and therefore any `AppHandle`) exists, so the base directory
/// decision has to be made that early too.
pub fn platform_app_data_dir() -> anyhow::Result<PathBuf> {
    dirs::data_dir()
        .map(|d| d.join(APP_IDENTIFIER))
        .ok_or_else(|| anyhow::anyhow!("could not determine the OS application data directory"))
}

/// The directories the webview may read through the asset protocol
/// (`convertFileSrc`): only the mods folder under the resolved data folder
/// (portable or app-data, whichever `decide_base_dir` picked), which is where
/// mod icons and option images live. Nothing else on disk is exposed.
///
/// Tauri canonicalizes every requested path before matching it against the
/// scope, so when the mods folder is reached through a symlink its
/// canonical form is allowed too. `tauri.conf.json` deliberately grants no
/// static scope (a relative pattern there, like the old `./**`, matched no
/// absolute path at all, which broke every mod image in release builds).
pub fn asset_scope_dirs(base_path: &Path) -> Vec<PathBuf> {
    let mods_dir = base_path.join(crate::commands::mods::MODS_DIRECTORY);
    let mods_dir: PathBuf = mods_dir.components().collect();
    let mut dirs = vec![mods_dir.clone()];
    if let Ok(canonical) = std::fs::canonicalize(&mods_dir) {
        // On Windows canonicalize adds a `\\?\` prefix; Tauri's scope
        // already matches both forms of whatever it's given.
        if canonical != mods_dir {
            dirs.push(canonical);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_app_data_when_nothing_portable_present() {
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();

        let decision = decide_base_dir(exe_dir.path(), app_data.path());

        assert_eq!(decision.kind, BaseDirKind::AppData);
        assert_eq!(decision.path, app_data.path());
    }

    #[test]
    fn uses_portable_when_marker_present_and_writable() {
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join(PORTABLE_MARKER_FILENAME), b"").unwrap();

        let decision = decide_base_dir(exe_dir.path(), app_data.path());

        assert_eq!(decision.kind, BaseDirKind::Portable);
        assert_eq!(decision.path, exe_dir.path());
    }

    #[test]
    fn uses_portable_when_legacy_mods_dir_present() {
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        std::fs::create_dir(exe_dir.path().join("mods")).unwrap();

        let decision = decide_base_dir(exe_dir.path(), app_data.path());

        assert_eq!(decision.kind, BaseDirKind::Portable);
    }

    #[test]
    fn uses_portable_when_legacy_settings_json_present() {
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join("settings.json"), b"{}").unwrap();

        let decision = decide_base_dir(exe_dir.path(), app_data.path());

        assert_eq!(decision.kind, BaseDirKind::Portable);
    }

    #[cfg(unix)]
    #[test]
    fn falls_back_to_app_data_when_portable_dir_is_not_writable() {
        use std::os::unix::fs::PermissionsExt;

        // Skip entirely when running as root (e.g. some CI/sandbox setups):
        // root ignores directory write permission bits, so this scenario is
        // unobservable and the test would be asserting the wrong thing.
        if unsafe { libc_geteuid() } == 0 {
            return;
        }

        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join(PORTABLE_MARKER_FILENAME), b"").unwrap();

        let mut perms = std::fs::metadata(exe_dir.path()).unwrap().permissions();
        perms.set_mode(0o555); // read + execute, no write
        std::fs::set_permissions(exe_dir.path(), perms.clone()).unwrap();

        let decision = decide_base_dir(exe_dir.path(), app_data.path());

        // Restore write permission so the tempdir can clean itself up.
        perms.set_mode(0o755);
        std::fs::set_permissions(exe_dir.path(), perms).unwrap();

        assert_eq!(decision.kind, BaseDirKind::AppData);
        assert_eq!(decision.path, app_data.path());
    }

    #[cfg(unix)]
    extern "C" {
        #[link_name = "geteuid"]
        fn libc_geteuid() -> u32;
    }

    #[test]
    fn resolve_exe_dir_prefers_appimage_env_var() {
        // SAFETY: test-only; no other test in this process reads/writes
        // this exact env var concurrently within the same assertion window.
        unsafe {
            std::env::set_var("APPIMAGE", "/opt/apps/DDMM-x86_64.AppImage");
        }
        let dir = resolve_exe_dir();
        unsafe {
            std::env::remove_var("APPIMAGE");
        }

        assert_eq!(dir, PathBuf::from("/opt/apps"));
    }

    #[test]
    fn asset_scope_is_only_the_mods_folder() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("mods")).unwrap();
        let dirs = asset_scope_dirs(base.path());
        let mods = std::fs::canonicalize(base.path().join("mods")).unwrap();
        assert!(dirs.iter().any(|d| d == &base.path().join("mods")));
        assert!(dirs.iter().any(|d| d == &mods));
        for d in &dirs {
            assert!(d.ends_with("mods"), "unexpected asset scope dir {d:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn asset_scope_includes_the_canonical_mods_folder_behind_a_symlink() {
        let real = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(real.path().join("mods")).unwrap();
        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("data");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        let dirs = asset_scope_dirs(&link);
        assert!(dirs.contains(&link.join("mods")));
        assert!(dirs.contains(&std::fs::canonicalize(real.path().join("mods")).unwrap()));
    }

    #[test]
    fn tauri_conf_grants_no_static_asset_scope() {
        // The asset scope is granted at runtime (asset_scope_dirs); a static
        // pattern here is either too broad or, if relative like the old
        // "./**", matches nothing and breaks every mod image.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let asset = &conf["app"]["security"]["assetProtocol"];
        assert_eq!(asset["enable"], serde_json::Value::Bool(true));
        assert_eq!(asset["scope"], serde_json::json!([]));
    }

    // ---- pointer files ------------------------------------------------

    struct Dirs {
        _root: tempfile::TempDir,
        exe: PathBuf,
        app_data: PathBuf,
        config: PathBuf,
        custom: PathBuf,
    }

    fn dirs() -> Dirs {
        let root = tempfile::tempdir().unwrap();
        let d = Dirs {
            exe: root.path().join("exe"),
            app_data: root.path().join("appdata"),
            config: root.path().join("config"),
            custom: root.path().join("custom"),
            _root: root,
        };
        for p in [&d.exe, &d.app_data, &d.config, &d.custom] {
            std::fs::create_dir_all(p).unwrap();
        }
        d
    }

    #[test]
    fn no_pointer_keeps_the_existing_defaults() {
        let d = dirs();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.path, d.app_data);
        assert!(!decision.pointed && decision.problem.is_none());
        assert_eq!(decision.default_path, d.app_data);
        assert_eq!(decision.pointer_file, d.config.join(POINTER_FILENAME));

        std::fs::write(d.exe.join(PORTABLE_MARKER_FILENAME), b"").unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.path, d.exe);
        assert_eq!(decision.kind, BaseDirKind::Portable);
        assert_eq!(decision.pointer_file, d.exe.join(POINTER_FILENAME));
    }

    #[test]
    fn installed_pointer_beats_the_app_data_default() {
        let d = dirs();
        write_pointer(&d.config.join(POINTER_FILENAME), &d.custom).unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.path, d.custom);
        assert_eq!(decision.kind, BaseDirKind::AppData);
        assert!(decision.pointed && decision.problem.is_none());
        assert_eq!(decision.default_path, d.app_data);
    }

    #[test]
    fn portable_folder_ignores_the_installed_pointer() {
        let d = dirs();
        std::fs::write(d.exe.join(PORTABLE_MARKER_FILENAME), b"").unwrap();
        write_pointer(&d.config.join(POINTER_FILENAME), &d.custom).unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.path, d.exe);
        assert!(!decision.pointed);
    }

    #[test]
    fn portable_pointer_beats_everything() {
        let d = dirs();
        std::fs::write(d.exe.join(PORTABLE_MARKER_FILENAME), b"").unwrap();
        std::fs::create_dir(d.exe.join("mods")).unwrap();
        let other = d._root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        write_pointer(&d.config.join(POINTER_FILENAME), &other).unwrap();
        write_pointer(&d.exe.join(POINTER_FILENAME), &d.custom).unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.path, d.custom);
        assert_eq!(decision.kind, BaseDirKind::Portable);
        assert_eq!(decision.default_path, d.exe);
        assert!(decision.pointed);
    }

    #[test]
    fn portable_pointer_alone_marks_the_folder_portable() {
        // A legacy portable copy (detected by its mods/ folder) whose data
        // was moved out: the pointer next to the exe keeps it portable.
        let d = dirs();
        write_pointer(&d.exe.join(POINTER_FILENAME), &d.custom).unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.kind, BaseDirKind::Portable);
        assert_eq!(decision.path, d.custom);
    }

    #[cfg(unix)]
    #[test]
    fn portable_pointer_works_from_a_read_only_exe_folder() {
        use std::os::unix::fs::PermissionsExt;
        let d = dirs();
        write_pointer(&d.exe.join(POINTER_FILENAME), &d.custom).unwrap();
        let mut perms = std::fs::metadata(&d.exe).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&d.exe, perms.clone()).unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        perms.set_mode(0o755);
        std::fs::set_permissions(&d.exe, perms).unwrap();
        assert_eq!(decision.path, d.custom);
        assert!(decision.problem.is_none());
    }

    #[test]
    fn pointed_folder_inside_the_portable_folder_is_stored_relative() {
        let d = dirs();
        let inside = d.exe.join("Data");
        std::fs::create_dir(&inside).unwrap();
        write_pointer(&d.exe.join(POINTER_FILENAME), &inside).unwrap();
        let raw: PointerFile =
            serde_json::from_slice(&std::fs::read(d.exe.join(POINTER_FILENAME)).unwrap()).unwrap();
        assert_eq!(raw.path, PathBuf::from("Data"));

        // Moving the whole portable folder keeps it working.
        let moved = d._root.path().join("moved-exe");
        std::fs::rename(&d.exe, &moved).unwrap();
        let decision = decide_data_dir(&moved, &d.app_data, &d.config);
        assert_eq!(decision.path, moved.join("Data"));
        assert!(decision.problem.is_none());
    }

    #[test]
    fn missing_pointed_folder_is_reported_and_never_created() {
        let d = dirs();
        let usb = d._root.path().join("usb-drive").join("DDMM Data");
        write_pointer(&d.config.join(POINTER_FILENAME), &usb).unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert_eq!(decision.problem, Some(DataDirProblem::Missing));
        assert_eq!(decision.path, usb);
        assert!(!usb.exists(), "deciding must never create the folder");

        // Plugged back in: fine again.
        std::fs::create_dir_all(&usb).unwrap();
        assert_eq!(decide_data_dir(&d.exe, &d.app_data, &d.config).problem, None);
    }

    #[test]
    fn unreadable_pointer_is_a_problem_not_a_silent_default() {
        let d = dirs();
        std::fs::write(d.config.join(POINTER_FILENAME), b"not json").unwrap();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert!(matches!(decision.problem, Some(DataDirProblem::BadPointer(_))));

        remove_pointer(&d.config.join(POINTER_FILENAME)).unwrap();
        remove_pointer(&d.config.join(POINTER_FILENAME)).unwrap(); // already gone: fine
        assert_eq!(decide_data_dir(&d.exe, &d.app_data, &d.config).problem, None);
    }
}
