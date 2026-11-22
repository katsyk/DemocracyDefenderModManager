//! What the startup recovery did, or couldn't do, that the user should
//! know about (see `commands::mods::recover_leftover_folders`).
//!
//! The recovery runs once per session, before the mod list is first read,
//! and collects these notices next to its log lines. They're kept in
//! [`crate::AppState`] for the session; the Mods page takes them once
//! (`take_startup_notices`) after it has loaded the mods and the profiles,
//! and shows them in one popup. A problem that persists (a hidden folder
//! DDMM can't put back) is found again, and so shown again, on every start
//! until it's resolved -- but only once per session.

use std::path::{Path, PathBuf};

use anyhow_tauri::{IntoTAResult, TAResult};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::AppState;

/// What happened. Each kind has its own text on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    /// An update or replace that DDMM was closed in the middle of: the new
    /// version was already in place, and this start wrote its profile
    /// entries.
    UpdateFinished,
    /// An update or replace stopped before the new version was in place:
    /// the mod is at the version it had (put back, if it had been set
    /// aside).
    UpdateUndone,
    /// Writing an unfinished update's profile entries reset option choices
    /// that no longer fit the new version (`profiles` names where).
    OptionsReset,
    /// An unfinished update's profile entries couldn't be written to
    /// profiles.json (another program has it open); the next start tries
    /// again.
    ProfilesRetry,
    /// An unfinished update's profile entries are kept for a later start:
    /// the new version's manifest.json can't be read.
    UpdatePending,
    /// A hidden `.adopt-…` folder (a wrapped folder being set up where it
    /// is) that DDMM couldn't put back or finish.
    AdoptLeft,
    /// A hidden `.update-backup-…` folder (the version an unfinished update
    /// set aside) that DDMM couldn't put back.
    BackupKept,
    /// A hidden `.update-…` folder (an unfinished update's new files) kept
    /// because it isn't the same as the installed mod.
    StagingKept,
    /// A hidden `.delete-…` folder without the record that says DDMM set
    /// it aside for deleting: never deleted.
    DeleteKept,
    /// An installed mod whose manifest.json can't be read: not listed.
    ModUnreadable,
}

/// One notice: what happened, to which mod, in which folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupNotice {
    pub kind: NoticeKind,
    /// The mod's name (its manifest's, else its folder's), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mod_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guid: Option<Uuid>,
    /// The folder it's about, shown in full; the page offers to open it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// The profiles it's about ([`NoticeKind::OptionsReset`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
}

impl StartupNotice {
    pub fn new(kind: NoticeKind) -> Self {
        Self { kind, mod_name: None, guid: None, folder: None, profiles: Vec::new() }
    }

    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.mod_name = Some(name.into());
        self
    }

    pub fn guid(mut self, guid: Uuid) -> Self {
        self.guid = Some(guid);
        self
    }

    pub fn folder(mut self, folder: impl Into<PathBuf>) -> Self {
        self.folder = Some(folder.into());
        self
    }

    pub fn profiles(mut self, profiles: Vec<String>) -> Self {
        self.profiles = profiles;
        self
    }
}

/// The notices of one start, without repeats: one per kind and mod (or
/// folder).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Notices(Vec<StartupNotice>);

impl Notices {
    pub fn push(&mut self, notice: StartupNotice) {
        let same = |n: &StartupNotice| {
            n.kind == notice.kind && n.guid == notice.guid && n.folder == notice.folder && n.mod_name == notice.mod_name
        };
        if !self.0.iter().any(same) {
            self.0.push(notice);
        }
    }

    /// Whether a notice already names `folder` (a mod skipped for its
    /// unreadable manifest that an update is waiting on needs only one).
    pub fn names_folder(&self, folder: &Path) -> bool {
        self.0.iter().any(|n| n.folder.as_deref() == Some(folder))
    }

    pub fn extend(&mut self, other: Notices) {
        for n in other.0 {
            self.push(n);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[StartupNotice] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<StartupNotice> {
        self.0
    }
}

/// This session's notices, once: the first call returns them, every later
/// one nothing (the page shows them once per session, however often it's
/// loaded again).
#[tauri::command]
pub fn take_startup_notices(state: State<'_, AppState>) -> Vec<StartupNotice> {
    state.take_startup_notices()
}

/// Open a folder a notice of this session named, in the file manager (or
/// the folder it was in, if it's gone since). Only those: the page can't
/// open anything else through this.
#[tauri::command]
pub fn open_startup_notice_folder(app: tauri::AppHandle, state: State<'_, AppState>, folder: PathBuf) -> TAResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let target = state.startup_notice_folder(&folder).ok_or_else(|| anyhow::anyhow!("{} isn't a folder DDMM told you about", folder.display())).into_ta_result()?;
    log::info!("Opening {:?}.", target);
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| anyhow::anyhow!("couldn't open {}: {e}", target.display()))
        .into_ta_result()
}

/// Where to open `folder` (see [`open_startup_notice_folder`]): itself,
/// else the folder it was in.
pub(crate) fn folder_to_open(folder: &Path) -> PathBuf {
    if folder.is_dir() {
        folder.to_path_buf()
    } else {
        folder.parent().map(Path::to_path_buf).unwrap_or_else(|| folder.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_notice_is_kept_once() {
        let mut notices = Notices::default();
        let n = StartupNotice::new(NoticeKind::AdoptLeft).named("Cool Mod").folder("/m/.adopt-1");
        notices.push(n.clone());
        notices.push(n.clone());
        notices.push(StartupNotice::new(NoticeKind::AdoptLeft).named("Cool Mod").folder("/m/.adopt-2"));
        assert_eq!(notices.as_slice().len(), 2);
    }

    #[test]
    fn it_serializes_for_the_page() {
        let n = StartupNotice::new(NoticeKind::OptionsReset).named("Cool Mod").profiles(vec!["Default".into()]);
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::json!({ "kind": "optionsReset", "modName": "Cool Mod", "profiles": ["Default"] })
        );
    }

    #[test]
    fn taking_them_empties_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let folder = dir.path().join("mods").join(".adopt-1");
        let mut notices = Notices::default();
        notices.push(StartupNotice::new(NoticeKind::AdoptLeft).folder(&folder));
        state.add_startup_notices(notices);
        assert_eq!(state.take_startup_notices().len(), 1);
        assert!(state.take_startup_notices().is_empty(), "once per session");
        assert_eq!(state.startup_notice_folder(&folder), Some(dir.path().join("mods")), "still openable after it was taken");
        assert_eq!(state.startup_notice_folder(&dir.path().join("mods")), None, "only folders a notice named");
    }
}
