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
fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".ddmm-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
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
}
