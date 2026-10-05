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

/// The folder an installed DDMM keeps its pointer file in when the OS
/// config folder is also where the data lives (see
/// [`installed_pointer_dir`]). Its own folder, next to (never inside) the
/// default data folder.
pub const POINTER_DIR_NAME: &str = "io.github.katsyk.ddmm.location";

/// Where an installed (non-portable) DDMM keeps its pointer file: always
/// outside the default data folder, so deleting that folder after moving
/// the data elsewhere can't lose the record of where the data went.
///
/// - Linux: `$XDG_CONFIG_HOME/<identifier>` (or `~/.config/...`), as it
///   always was; the data is in `~/.local/share`.
/// - Windows: `%APPDATA%\io.github.katsyk.ddmm.location`, next to the
///   (Roaming) default data folder, so it roams with it.
/// - macOS: `~/Library/Application Support/io.github.katsyk.ddmm.location`,
///   next to the data folder.
///
/// Earlier versions used `<config dir>/<identifier>`, which on Windows and
/// macOS *is* the default data folder. That old file ([`legacy_pointer_dir`])
/// is kept up to date as a mirror (see [`commit_pointer`]), so going back to
/// an earlier version still finds the data.
pub fn installed_pointer_dir() -> Option<PathBuf> {
    pointer_dir_for(dirs::config_dir(), dirs::data_dir())
}

/// [`installed_pointer_dir`] from the OS config and data folders (a
/// function of them, for tests).
pub fn pointer_dir_for(config: Option<PathBuf>, data: Option<PathBuf>) -> Option<PathBuf> {
    let config = config?;
    let old = config.join(APP_IDENTIFIER);
    if data.map(|d| d.join(APP_IDENTIFIER)).as_ref() != Some(&old) {
        return Some(old);
    }
    Some(config.join(POINTER_DIR_NAME))
}

/// Where earlier versions kept an installed copy's pointer file, when that
/// isn't [`installed_pointer_dir`] (Windows and macOS).
pub fn legacy_pointer_dir() -> Option<PathBuf> {
    legacy_pointer_dir_for(dirs::config_dir(), dirs::data_dir())
}

fn legacy_pointer_dir_for(config: Option<PathBuf>, data: Option<PathBuf>) -> Option<PathBuf> {
    let old = config.clone()?.join(APP_IDENTIFIER);
    (Some(&old) != pointer_dir_for(config, data).as_ref()).then_some(old)
}

/// Whether deciding the data folder may copy an old pointer file to its new
/// place ([`PointerAccess::Migrate`], the app) or must only read
/// ([`PointerAccess::ReadOnly`], the browser's native-messaging host, which
/// runs alongside the app and must never write where it keeps its data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAccess {
    Migrate,
    ReadOnly,
}

/// Copy the old pointer to `new` if `new` doesn't exist yet. The old file
/// is never changed or removed. An unreadable old pointer is copied as it
/// is, so the recovery screen still says so.
fn copy_pointer_if_missing(old: &Path, new: &Path) -> anyhow::Result<()> {
    if pointer_present(new) {
        return Ok(());
    }
    match read_pointer(old) {
        // Resolved against its old folder and stored absolute (a relative
        // path would now be relative to the wrong folder).
        Ok(Some(target)) => write_pointer_raw(new, &PointerFile { version: POINTER_VERSION, path: target }),
        Ok(None) => Ok(()),
        Err(_) => write_bytes_durably(new, &crate::fs_util::read_file_blocking(old)?),
    }
}

/// Whether a pointer file is there. One whose existence can't be checked
/// (still busy after retrying, see [`crate::fs_util::file_exists_blocking`])
/// counts as there: it is then read, and an error reading it is reported
/// (the recovery screen), never taken as "no pointer" (the default folder).
fn pointer_present(pointer_file: &Path) -> bool {
    crate::fs_util::file_exists_blocking(pointer_file).unwrap_or(true)
}

/// Which installed pointer file to read: `new` (authoritative) or the
/// earlier versions' `old` one, plus a note for the log when they disagree.
///
/// - Only `old` exists: with [`PointerAccess::Migrate`] it's copied to
///   `new` first. If that fails (or another process is doing the same at
///   the same moment), whatever `new` holds afterwards is used, else `old`.
/// - Both exist and disagree: the one whose folder exists wins; if both
///   exist, the more recently written file. Neither is ever deleted.
fn installed_pointer_to_read(new: &Path, old: Option<&Path>, access: PointerAccess) -> (PathBuf, Option<String>) {
    let Some(old) = old.filter(|o| *o != new && pointer_present(o)) else { return (new.to_path_buf(), None) };
    if !pointer_present(new) {
        if access == PointerAccess::Migrate {
            if let Err(e) = copy_pointer_if_missing(old, new) {
                eprintln!("warning: couldn't copy {} to {}: {e:#}", old.display(), new.display());
            }
            if pointer_present(new) {
                return (new.to_path_buf(), None);
            }
        }
        return (old.to_path_buf(), None);
    }
    let (from_new, from_old) = (read_pointer(new), read_pointer(old));
    if let (Ok(a), Ok(b)) = (&from_new, &from_old) {
        if a == b {
            return (new.to_path_buf(), None);
        }
    }
    let rank = |r: &Result<Option<PathBuf>, String>| match r {
        Ok(Some(p)) if p.is_dir() => 2,
        Ok(Some(_)) => 1,
        _ => 0,
    };
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let (rank_new, rank_old) = (rank(&from_new), rank(&from_old));
    let pick_old = rank_old > rank_new || (rank_new == 2 && rank_old == 2 && modified(old) > modified(new));
    let chosen = if pick_old { old } else { new };
    let note = format!(
        "{} ({:?}) and {} ({:?}) disagree; using {}",
        new.display(),
        from_new,
        old.display(),
        from_old,
        chosen.display()
    );
    eprintln!("warning: {note}");
    (chosen.to_path_buf(), Some(note))
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
    /// An earlier versions' copy of the pointer (installed copies on
    /// Windows and macOS) that every change is mirrored to, best effort,
    /// so going back to such a version still finds the data.
    pub mirror_pointer_file: Option<PathBuf>,
    /// `path` came from a pointer file.
    pub pointed: bool,
    pub problem: Option<DataDirProblem>,
}

/// Where the log files go: `logs/` in the data folder, or a folder in the
/// temp dir on the recovery screen (the data folder is missing then, and
/// must not be created just to hold a log).
pub fn log_dir(decision: &DataDirDecision) -> PathBuf {
    if decision.problem.is_some() {
        std::env::temp_dir().join(format!("{APP_IDENTIFIER}-logs"))
    } else {
        decision.path.join("logs")
    }
}

/// Read a pointer file. `Ok(None)` only when there is none (`NotFound`).
///
/// On Windows a pointer that another process is replacing at this moment
/// (the app migrating or moving the data folder, a second launch, the
/// browser helper reading alongside) can't be opened for a few
/// milliseconds: access denied or a sharing violation. That is retried
/// briefly; if it persists it is an error (the recovery screen), never "no
/// pointer", which would silently use the default folder.
pub fn read_pointer(pointer_file: &Path) -> Result<Option<PathBuf>, String> {
    let data = match crate::fs_util::read_file_blocking(pointer_file) {
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
/// folder is stored relative to it (see [`PointerFile`]). The file (and, on
/// Unix, its folder) is synced to disk before this returns: a move deletes
/// the old data right after, and a crash in between must not leave DDMM
/// pointing nowhere.
pub fn write_pointer(pointer_file: &Path, target: &Path) -> anyhow::Result<()> {
    let dir = pointer_file.parent().unwrap_or(Path::new("."));
    let _ = std::fs::create_dir_all(dir);
    let stored = relative_if_inside(target, dir).unwrap_or_else(|| target.to_path_buf());
    write_pointer_raw(pointer_file, &PointerFile { version: POINTER_VERSION, path: stored })
}

fn write_pointer_raw(pointer_file: &Path, contents: &PointerFile) -> anyhow::Result<()> {
    write_bytes_durably(pointer_file, &serde_json::to_vec_pretty(contents)?)
}

/// Write a pointer file atomically and durably, retrying while another
/// program has it open: [`crate::fs_util::replace_file_durably_blocking`].
/// Its temporary file has a unique name, since the app and the browser
/// helper (or two app launches) may write at the same moment.
fn write_bytes_durably(pointer_file: &Path, data: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;
    let dir = pointer_file.parent().unwrap_or(Path::new("."));
    let existed = dir.is_dir();
    std::fs::create_dir_all(dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    if !existed {
        // The new folder's own entry must be durable too.
        sync_dir(dir.parent().unwrap_or(Path::new(".")));
    }
    crate::fs_util::replace_file_durably_blocking(pointer_file, data)
}

/// Make a rename or removal in `dir` durable, where the OS allows syncing a
/// folder (Unix). On Windows, NTFS journals the rename itself.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Remove a pointer file ("Reset to default"). Already gone is fine.
pub fn remove_pointer(pointer_file: &Path) -> anyhow::Result<()> {
    use crate::fs_util::{is_transient_file_error, retry_blocking, REPLACE_RETRY_DELAYS};
    match retry_blocking(REPLACE_RETRY_DELAYS, is_transient_file_error, || std::fs::remove_file(pointer_file)) {
        Ok(()) => {
            sync_dir(pointer_file.parent().unwrap_or(Path::new(".")));
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("couldn't remove {}: {e}", pointer_file.display())),
    }
}

/// Record `target` as the chosen data folder in `decision`'s pointer file,
/// and mirror it (best effort) to the earlier versions' copy when that
/// one's folder exists (see [`DataDirDecision::mirror_pointer_file`]).
pub fn commit_pointer(decision: &DataDirDecision, target: &Path) -> anyhow::Result<()> {
    write_pointer(&decision.pointer_file, target)?;
    if let Some(mirror) = decision.mirror_pointer_file.as_deref().filter(|m| m.parent().is_some_and(Path::is_dir)) {
        if let Err(e) = write_pointer(mirror, target) {
            log::warn!("Couldn't update the older copy of the data-folder location ({mirror:?}): {e:#}");
        }
    }
    Ok(())
}

/// Forget the chosen data folder: remove `decision`'s pointer file and
/// (best effort) its mirror.
pub fn clear_pointer(decision: &DataDirDecision) -> anyhow::Result<()> {
    remove_pointer(&decision.pointer_file)?;
    if let Some(mirror) = &decision.mirror_pointer_file {
        if let Err(e) = remove_pointer(mirror) {
            log::warn!("Couldn't remove the older copy of the data-folder location: {e:#}");
        }
    }
    Ok(())
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
    decide_data_dir_with(exe_dir, app_data_dir, installed_pointer_dir, None, PointerAccess::Migrate)
}

/// [`decide_data_dir`], also knowing where earlier versions kept the
/// installed pointer (`legacy_pointer_dir`, see [`installed_pointer_to_read`]).
/// That's only looked at -- and only copied with [`PointerAccess::Migrate`]
/// -- when the decision gets as far as the installed pointer: a portable
/// copy never touches it.
pub fn decide_data_dir_with(
    exe_dir: &Path,
    app_data_dir: &Path,
    installed_pointer_dir: &Path,
    legacy_pointer_dir: Option<&Path>,
    access: PointerAccess,
) -> DataDirDecision {
    let portable_pointer = exe_dir.join(POINTER_FILENAME);
    let installed_pointer = installed_pointer_dir.join(POINTER_FILENAME);
    let legacy_pointer = legacy_pointer_dir.map(|d| d.join(POINTER_FILENAME)).filter(|p| *p != installed_pointer);

    let pointed = |read_from: &Path,
                   pointer_file: PathBuf,
                   mirror: Option<PathBuf>,
                   kind: BaseDirKind,
                   default_path: &Path,
                   note: Option<String>|
     -> Option<DataDirDecision> {
        let note = note.map(|n| format!("; {n}")).unwrap_or_default();
        match read_pointer(read_from) {
            Ok(None) => None,
            Ok(Some(path)) => {
                let problem = (!path.is_dir()).then_some(DataDirProblem::Missing);
                Some(DataDirDecision {
                    path,
                    kind,
                    reason: format!("chosen in Settings ({}){note}", read_from.display()),
                    default_path: default_path.to_path_buf(),
                    pointer_file,
                    mirror_pointer_file: mirror,
                    pointed: true,
                    problem,
                })
            }
            Err(message) => Some(DataDirDecision {
                path: default_path.to_path_buf(),
                kind,
                reason: format!("unreadable data-location file{note}"),
                default_path: default_path.to_path_buf(),
                pointer_file,
                mirror_pointer_file: mirror,
                pointed: true,
                problem: Some(DataDirProblem::BadPointer(message)),
            }),
        }
    };

    if let Some(d) = pointed(&portable_pointer, portable_pointer.clone(), None, BaseDirKind::Portable, exe_dir, None) {
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
            mirror_pointer_file: None,
            pointed: false,
            problem: None,
        };
    }

    let (read_from, note) = installed_pointer_to_read(&installed_pointer, legacy_pointer.as_deref(), access);
    if let Some(d) =
        pointed(&read_from, installed_pointer.clone(), legacy_pointer.clone(), BaseDirKind::AppData, app_data_dir, note)
    {
        return d;
    }

    DataDirDecision {
        path: base.path,
        kind: base.kind,
        reason: base.reason.to_string(),
        default_path: app_data_dir.to_path_buf(),
        pointer_file: installed_pointer,
        mirror_pointer_file: legacy_pointer,
        pointed: false,
        problem: None,
    }
}

/// [`decide_data_dir`] for this machine (the real executable, app data and
/// config folders), as the app does it at startup: an earlier version's
/// pointer is copied to its new place if needed.
pub fn decide_data_dir_for_this_machine() -> DataDirDecision {
    decide_for_this_machine(PointerAccess::Migrate)
}

/// The same decision for the browser's native-messaging host, so both
/// always agree on where `bridge.json` is -- but strictly read-only: the
/// host reads the new pointer, else the old one, and never writes.
pub fn decide_data_dir_for_this_machine_read_only() -> DataDirDecision {
    decide_for_this_machine(PointerAccess::ReadOnly)
}

fn decide_for_this_machine(access: PointerAccess) -> DataDirDecision {
    let exe_dir = resolve_exe_dir();
    let app_data_dir = platform_app_data_dir().unwrap_or_else(|e| {
        eprintln!("warning: {e}; falling back to the executable directory for app data");
        exe_dir.clone()
    });
    let pointer_dir = installed_pointer_dir().unwrap_or_else(|| app_data_dir.clone());
    decide_data_dir_with(&exe_dir, &app_data_dir, &pointer_dir, legacy_pointer_dir().as_deref(), access)
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
    fn log_dir_is_in_the_data_folder_unless_it_is_missing() {
        let d = dirs();
        let custom = d.custom.join("DDMM [data] v1.2");
        let mut decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        decision.path = custom.clone();
        assert_eq!(log_dir(&decision), custom.join("logs"));
        decision.problem = Some(DataDirProblem::Missing);
        assert_eq!(log_dir(&decision), std::env::temp_dir().join(format!("{APP_IDENTIFIER}-logs")));
    }

    #[test]
    #[cfg(not(windows))] // see the `tauri` dev-dependency in Cargo.toml
    fn asset_scope_allows_mod_images_and_nothing_else() {
        // What the asset protocol is asked for: images in mod folders whose
        // names have spaces, dots, brackets and timestamps, under default
        // and custom data folders (also with glob characters in them).
        let root = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_app();
        for base in [
            root.path().join("AppData").join("Roaming").join(APP_IDENTIFIER),
            root.path().join("My Mods [HD2] {v1.2}").join("DDMM Data"),
        ] {
            let scope = tauri::scope::fs::Scope::new(&app, &Default::default()).unwrap();
            std::fs::create_dir_all(base.join("mods")).unwrap();
            for dir in asset_scope_dirs(&base) {
                scope.allow_directory(&dir, true).unwrap();
            }
            for (folder, file) in [
                ("AbdoStyled Ironclad Democracy 16609 1.0.0 2026-09-27T03-15Z sZaXo0pG9", "Screenshot 2026-09-26 131540.png"),
                ("Abdo Styled Crewmates-9947-1-1-9-1778123166", "icon.png"),
                ("Leopard Super Earth V1.5 [c85057a93f7d]", "images/thumbnail.png"),
                ("First Person (experimental)", "a*b?.png"),
            ] {
                let image = base.join("mods").join(folder).join(file);
                std::fs::create_dir_all(image.parent().unwrap()).unwrap();
                std::fs::write(&image, b"png").unwrap();
                assert!(scope.is_allowed(&image), "{image:?} should be allowed");
            }
            std::fs::write(base.join("settings.json"), b"{}").unwrap();
            assert!(!scope.is_allowed(base.join("settings.json")));
            assert!(!scope.is_allowed(base.join("mods").join("..").join("settings.json")));
            assert!(!scope.is_allowed(root.path().join("other.png")));
        }
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

    /// On Windows and macOS the OS config folder is the data folder's
    /// parent too; the pointer must never land inside the data folder.
    #[test]
    fn the_installed_pointer_is_never_inside_the_default_data_folder() {
        // Windows (Roaming for both) / macOS shape: config == data.
        let roaming = PathBuf::from("C:/Users/u/AppData/Roaming");
        let dir = pointer_dir_for(Some(roaming.clone()), Some(roaming.clone())).unwrap();
        assert_eq!(dir, roaming.join(POINTER_DIR_NAME), "a Roaming sibling of the data folder");
        assert!(!dir.starts_with(roaming.join(APP_IDENTIFIER)));
        assert_eq!(legacy_pointer_dir_for(Some(roaming.clone()), Some(roaming.clone())), Some(roaming.join(APP_IDENTIFIER)));
        // Linux: unchanged, and no older location to look at.
        let (config, data) = (PathBuf::from("/home/u/.config"), PathBuf::from("/home/u/.local/share"));
        assert_eq!(pointer_dir_for(Some(config.clone()), Some(data.clone())), Some(config.join(APP_IDENTIFIER)));
        assert_eq!(legacy_pointer_dir_for(Some(config), Some(data)), None);
    }

    /// The Windows layout: the old pointer sits inside the default data
    /// folder, the new one in a sibling folder.
    struct Layout {
        d: Dirs,
        new_dir: PathBuf,
        old_dir: PathBuf,
    }

    fn layout() -> Layout {
        let d = dirs();
        let new_dir = d._root.path().join(POINTER_DIR_NAME);
        let old_dir = d.app_data.clone();
        Layout { d, new_dir, old_dir }
    }

    impl Layout {
        fn decide(&self, access: PointerAccess) -> DataDirDecision {
            decide_data_dir_with(&self.d.exe, &self.d.app_data, &self.new_dir, Some(&self.old_dir), access)
        }
        fn new_file(&self) -> PathBuf {
            self.new_dir.join(POINTER_FILENAME)
        }
        fn old_file(&self) -> PathBuf {
            self.old_dir.join(POINTER_FILENAME)
        }
    }

    /// Migration copies, never moves: the old pointer stays for a
    /// downgrade, and later changes are mirrored to it.
    #[test]
    fn migration_copies_the_old_pointer_and_changes_are_mirrored_to_it() {
        let l = layout();
        write_pointer(&l.old_file(), &l.d.custom).unwrap();
        let decision = l.decide(PointerAccess::Migrate);
        assert_eq!(decision.path, l.d.custom);
        assert!(decision.pointed && decision.problem.is_none());
        assert_eq!(decision.pointer_file, l.new_file());
        assert_eq!(decision.mirror_pointer_file, Some(l.old_file()));
        assert_eq!(read_pointer(&l.new_file()).unwrap(), Some(l.d.custom.clone()));
        assert_eq!(read_pointer(&l.old_file()).unwrap(), Some(l.d.custom.clone()), "the old one is kept");

        // A later move updates both, so an earlier version (reading only
        // the old file) follows along.
        let other = l.d._root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        commit_pointer(&decision, &other).unwrap();
        assert_eq!(read_pointer(&l.new_file()).unwrap(), Some(other.clone()));
        assert_eq!(read_pointer(&l.old_file()).unwrap(), Some(other.clone()));
        assert_eq!(l.decide(PointerAccess::Migrate).path, other);

        // Reset clears both, so a stale old copy can't be copied back in.
        clear_pointer(&l.decide(PointerAccess::Migrate)).unwrap();
        assert!(!l.new_file().exists() && !l.old_file().exists());
        let decision = l.decide(PointerAccess::Migrate);
        assert!(!decision.pointed);
        assert_eq!(decision.path, l.d.app_data);

        // Deleting the whole old default folder after a move loses
        // nothing, and a missing target is the recovery screen.
        commit_pointer(&decision, &other).unwrap();
        std::fs::remove_dir_all(&l.old_dir).unwrap();
        assert_eq!(l.decide(PointerAccess::Migrate).path, other);
        // No mirror when its folder is gone (nothing is recreated there).
        commit_pointer(&l.decide(PointerAccess::Migrate), &l.d.custom).unwrap();
        assert!(!l.old_dir.exists());
        std::fs::remove_dir_all(&l.d.custom).unwrap();
        assert_eq!(l.decide(PointerAccess::Migrate).problem, Some(DataDirProblem::Missing));
    }

    /// The browser helper only reads: new pointer, else the old one.
    #[test]
    fn read_only_access_never_writes() {
        let l = layout();
        write_pointer(&l.old_file(), &l.d.custom).unwrap();
        let decision = l.decide(PointerAccess::ReadOnly);
        assert_eq!(decision.path, l.d.custom);
        assert!(!l.new_dir.exists(), "read-only must not create the new pointer");
        assert!(l.old_file().is_file());
    }

    /// Two processes (two launches, or the app and the browser helper)
    /// deciding at the same moment must all find the data -- never the
    /// default folder, and never a recovery screen. On Windows a pointer
    /// another thread is renaming into place can't be opened for a moment
    /// (access denied); that used to be taken as an unreadable pointer, and
    /// with it the default folder.
    #[test]
    fn concurrent_migrations_all_find_the_data() {
        for _ in 0..200 {
            let l = std::sync::Arc::new(layout());
            write_pointer(&l.old_file(), &l.d.custom).unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let (l, barrier) = (l.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        let access = if i % 2 == 0 { PointerAccess::Migrate } else { PointerAccess::ReadOnly };
                        l.decide(access)
                    })
                })
                .collect();
            for h in handles {
                let decision = h.join().unwrap();
                assert_eq!((&decision.path, &decision.problem), (&l.d.custom, &None), "{}", decision.reason);
            }
            assert_eq!(read_pointer(&l.new_file()).unwrap(), Some(l.d.custom.clone()));
            let leftovers: Vec<_> = std::fs::read_dir(&l.new_dir).unwrap().flatten().map(|e| e.file_name()).collect();
            assert_eq!(leftovers.len(), 1, "no temp files left behind: {leftovers:?}");
        }
    }

    /// The app rewriting both pointers (a move, or a second launch
    /// migrating) while others decide: every reader still finds the data.
    #[test]
    fn deciding_while_the_pointers_are_rewritten_always_finds_the_data() {
        let l = std::sync::Arc::new(layout());
        write_pointer(&l.old_file(), &l.d.custom).unwrap();
        let decision = std::sync::Arc::new(l.decide(PointerAccess::Migrate));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (0..3)
            .map(|_| {
                let (l, decision, stop) = (l.clone(), decision.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut writes = 0;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        commit_pointer(&decision, &l.d.custom).unwrap();
                        writes += 1;
                    }
                    writes
                })
            })
            .collect();
        let readers: Vec<_> = (0..5)
            .map(|i| {
                let l = l.clone();
                std::thread::spawn(move || {
                    let access = if i % 2 == 0 { PointerAccess::Migrate } else { PointerAccess::ReadOnly };
                    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
                    let mut reads = 0;
                    while std::time::Instant::now() < deadline || reads < 50 {
                        let decision = l.decide(access);
                        assert_eq!((&decision.path, &decision.problem), (&l.d.custom, &None), "{}", decision.reason);
                        reads += 1;
                    }
                })
            })
            .collect();
        for r in readers {
            r.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for w in writers {
            assert!(w.join().unwrap() > 0);
        }
        for dir in [&l.new_dir, &l.old_dir] {
            let temps: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".tmp"))
                .collect();
            assert!(temps.is_empty(), "no temp files left behind: {temps:?}");
        }
    }

    /// A pointer another program holds open without sharing (an antivirus
    /// scan, a sync client) is waited for briefly; one that stays locked is
    /// the recovery screen, never the default folder.
    #[cfg(windows)]
    #[test]
    fn a_locked_pointer_is_waited_for_and_then_reported_never_ignored() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = dirs();
        let pointer = d.config.join(POINTER_FILENAME);
        write_pointer(&pointer, &d.custom).unwrap();
        let lock = || std::fs::OpenOptions::new().read(true).share_mode(0).open(&pointer).unwrap();

        let held = lock();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(held);
        });
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        release.join().unwrap();
        assert_eq!((&decision.path, &decision.problem), (&d.custom, &None), "{}", decision.reason);

        let _held = lock();
        let decision = decide_data_dir(&d.exe, &d.app_data, &d.config);
        assert!(matches!(decision.problem, Some(DataDirProblem::BadPointer(_))), "{decision:?}");
        assert!(decision.pointed);
    }

    /// Both pointers exist and disagree: the one whose folder exists wins,
    /// then the newer file; neither is deleted.
    #[test]
    fn disagreeing_pointers_prefer_the_one_that_works() {
        let l = layout();
        let gone = l.d._root.path().join("gone");
        write_pointer(&l.new_file(), &gone).unwrap();
        write_pointer(&l.old_file(), &l.d.custom).unwrap();
        let decision = l.decide(PointerAccess::Migrate);
        assert_eq!(decision.path, l.d.custom);
        assert!(decision.problem.is_none());
        assert!(decision.reason.contains("disagree"), "{}", decision.reason);
        assert!(l.new_file().is_file() && l.old_file().is_file());

        // Both folders exist: the more recently written pointer wins.
        let other = l.d._root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        write_pointer(&l.new_file(), &other).unwrap();
        let set = |p: &Path, secs| {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
                .unwrap()
        };
        set(&l.new_file(), 1_000_000);
        set(&l.old_file(), 2_000_000);
        assert_eq!(l.decide(PointerAccess::Migrate).path, l.d.custom);
        set(&l.new_file(), 3_000_000);
        assert_eq!(l.decide(PointerAccess::Migrate).path, other);
        assert!(l.new_file().is_file() && l.old_file().is_file());

        // An unreadable old copy doesn't beat a good new one.
        std::fs::write(l.old_file(), b"not json").unwrap();
        assert_eq!(l.decide(PointerAccess::Migrate).path, other);
    }

    /// A portable copy never reads, copies or changes the installed
    /// pointers.
    #[test]
    fn a_portable_copy_leaves_the_installed_pointers_alone() {
        let l = layout();
        std::fs::write(l.d.exe.join(PORTABLE_MARKER_FILENAME), b"").unwrap();
        write_pointer(&l.old_file(), &l.d.custom).unwrap();
        let before = std::fs::read(l.old_file()).unwrap();
        let decision = l.decide(PointerAccess::Migrate);
        assert_eq!(decision.kind, BaseDirKind::Portable);
        assert_eq!(decision.path, l.d.exe);
        assert!(decision.mirror_pointer_file.is_none());
        assert!(!l.new_dir.exists(), "no migration for a portable copy");
        assert_eq!(std::fs::read(l.old_file()).unwrap(), before);
    }

    #[test]
    fn a_broken_old_pointer_is_carried_over_to_the_recovery_screen() {
        let l = layout();
        std::fs::write(l.old_file(), b"not json").unwrap();
        let decision = l.decide(PointerAccess::Migrate);
        assert!(matches!(decision.problem, Some(DataDirProblem::BadPointer(_))));
        assert_eq!(std::fs::read(l.new_file()).unwrap(), b"not json");
        assert!(l.old_file().is_file());

        // A relative path (data inside the old folder) is stored resolved.
        let l = layout();
        let inside = l.old_dir.join("Moved");
        std::fs::create_dir_all(&inside).unwrap();
        write_pointer(&l.old_file(), &inside).unwrap();
        l.decide(PointerAccess::Migrate);
        let got = read_pointer(&l.new_file()).unwrap().unwrap();
        assert!(got.is_absolute());
        assert_eq!(std::fs::canonicalize(got).unwrap(), std::fs::canonicalize(&inside).unwrap());
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
