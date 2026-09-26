//! Bulk import: bring hundreds of mods over from another mod manager's
//! folder, or from a folder full of downloaded archives, without adding
//! them one by one.
//!
//! Three steps, each its own function so they can be tested without Tauri
//! (the commands live in `commands::import`):
//!
//! 1. [`detect_sources`] looks for the data folders of the mod managers
//!    people move here from (see `docs/development/importing-from-other-managers.md`
//!    for what each one stores where) and for the Downloads folder.
//! 2. [`scan`] lists what's in a folder (or in a list of picked files):
//!    every archive, and every folder that is a mod (has a `manifest.json`
//!    or Helldivers 2 patch files). It reads each one -- without extracting
//!    anything -- for its name, size, manifest and Nexus Mods id, then
//!    marks what's already in DDMM, what's a duplicate, what's an older
//!    copy of the same file, what isn't a Helldivers 2 mod and what can't
//!    be read.
//! 3. [`run_import`] installs the chosen items one after another through
//!    the normal install code (same archive hardening, same overlap checks)
//!    with progress and cancel, keeps going past failures, and records the
//!    Nexus id/version/file and the archive's fingerprint in each mod's
//!    `.hd2mm-origin.json` so update checks and re-imports work.
//!
//! The source is only ever read: nothing in another manager's folder or in
//! the user's downloads is written, moved or deleted, and a source that is,
//! contains, or is inside DDMM's data folder is refused outright.
//!
//! Nothing here downloads anything, from Nexus Mods or anywhere else.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod layouts;

use crate::{
    archive::Archive,
    commands::mods::{install_from_archive_as, install_from_folder_as, MODS_DIRECTORY},
    fs_util::{path_overlap, PathOverlap},
    models::{
        manifest::{Manifest, Source},
        profile::Config,
        Mod,
    },
    sources::{self, ArchiveFingerprint, InstalledFile, OriginSidecar, ORIGIN_SIDECAR_FILE},
    utils::is_patch_filename,
    AppState,
};

/// How deep [`scan`] looks for archives and mod folders below the folder
/// the user picked (a Downloads folder with a "HD2 mods/Armor" subfolder
/// is found; the rest of the disk isn't).
pub const MAX_SCAN_DEPTH: usize = 3;
/// How deep inside a folder patch files still make it "a mod".
const PATCH_SEARCH_DEPTH: usize = 5;
/// A scan stops listing after this many items (no real mod collection is
/// anywhere near it; this only protects against picking `C:\`).
pub const MAX_SCAN_ITEMS: usize = 5000;
/// Kept free on the drive beyond what the import itself needs.
pub const FREE_SPACE_MARGIN: u64 = 256 * 1024 * 1024;

const ARCHIVE_EXTENSIONS: [&str; 3] = ["zip", "7z", "rar"];
/// Folders that are never mods themselves and never hold any worth
/// importing (a manager's own cache/temp/log folders).
const SKIPPED_DIR_NAMES: [&str; 6] = ["logs", "temp", "tmp", "cache", ".git", "__MACOSX"];

// ---------------------------------------------------------------------------
// Source detection
// ---------------------------------------------------------------------------

/// What kind of folder a detected source is. Only ever shown to the user
/// through neutral wording ("another mod manager's mods", "downloads");
/// never by product name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// Another mod manager's folder of installed (unpacked) mods.
    ManagerMods,
    /// Another mod manager's folder of downloaded archives.
    ManagerDownloads,
    /// The user's Downloads folder (from DDMM's settings, or the OS one).
    Downloads,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DetectedSource {
    pub kind: SourceKind,
    pub path: PathBuf,
    /// Archives plus mod folders directly found there (a quick count, not
    /// a full scan).
    pub count: usize,
}

/// Where to look, per OS folder. Code comments may name the tools; the UI
/// never does. See `docs/development/importing-from-other-managers.md`.
#[derive(Debug, Clone, Default)]
pub struct KnownDirs {
    /// `%APPDATA%` (Roaming) on Windows, `~/.config` on Linux.
    pub config: Option<PathBuf>,
    /// `%LOCALAPPDATA%` on Windows, `~/.local/share` on Linux.
    pub local_data: Option<PathBuf>,
    /// The OS Downloads folder.
    pub downloads: Option<PathBuf>,
    /// DDMM's configured downloads folder (Settings), if different.
    pub settings_downloads: Option<PathBuf>,
}

impl KnownDirs {
    pub fn for_this_machine(settings_downloads: Option<PathBuf>) -> Self {
        KnownDirs {
            config: dirs::config_dir(),
            local_data: dirs::data_local_dir(),
            downloads: dirs::download_dir(),
            settings_downloads,
        }
    }
}

/// Every candidate folder, most specific first. Missing ones are dropped by
/// [`detect_sources`].
pub fn candidate_sources(dirs: &KnownDirs) -> Vec<(SourceKind, PathBuf)> {
    let mut out = Vec::new();
    // HD2 Arsenal: `%LOCALAPPDATA%\hd2arsenal` on Windows (Electron's
    // appData with "Roaming" swapped for "Local"), `~/.config/hd2arsenal`
    // on Linux, where that swap does nothing.
    if let Some(local) = &dirs.local_data {
        out.push((SourceKind::ManagerMods, local.join("hd2arsenal")));
    }
    if let Some(config) = &dirs.config {
        out.push((SourceKind::ManagerMods, config.join("hd2arsenal")));
    }
    if let Some(local) = &dirs.local_data {
        // Helldivers 2 Mod Manager (teutinsa; the project DDMM is derived
        // from) keeps its storage -- one folder per mod, each with a
        // manifest.json, plus profiles.json next to them -- in
        // `%LOCALAPPDATA%\Helldivers2ModManager` by default.
        out.push((SourceKind::ManagerMods, local.join("Helldivers2ModManager").join("Mods")));
        out.push((SourceKind::ManagerMods, local.join("Helldivers2ModManager")));
    }
    if let Some(config) = &dirs.config {
        // Its 2024 predecessor, in Roaming.
        out.push((SourceKind::ManagerMods, config.join("HD2ModManager").join("Mods")));
        // Vortex: staging folder (one unpacked folder per mod, named after
        // the archive) and its download folder (the original archives).
        out.push((SourceKind::ManagerMods, config.join("Vortex").join("helldivers2").join("mods")));
        out.push((SourceKind::ManagerDownloads, config.join("Vortex").join("downloads").join("helldivers2")));
    }
    if let Some(d) = &dirs.settings_downloads {
        out.push((SourceKind::Downloads, d.clone()));
    }
    if let Some(d) = &dirs.downloads {
        out.push((SourceKind::Downloads, d.clone()));
    }
    out
}

/// The candidate folders that exist, hold at least one importable item and
/// don't overlap DDMM's data folder (`base_path`), each listed once.
pub fn detect_sources(dirs: &KnownDirs, base_path: &Path) -> Vec<DetectedSource> {
    let mut out: Vec<DetectedSource> = Vec::new();
    for (kind, path) in candidate_sources(dirs) {
        if !path.is_dir() {
            continue;
        }
        if out.iter().any(|d| matches!(path_overlap(&d.path, &path), Ok(Some(PathOverlap::Same)))) {
            continue;
        }
        if !matches!(path_overlap(&path, base_path), Ok(None)) {
            continue;
        }
        let count = count_items(&path);
        if count == 0 {
            continue;
        }
        // The storage root and its `Mods` subfolder are the same source;
        // keep whichever came first (the more specific one).
        if out.iter().any(|d| d.kind == kind && matches!(path_overlap(&path, &d.path), Ok(Some(_)))) {
            continue;
        }
        out.push(DetectedSource { kind, path, count });
    }
    out
}

/// How many mods a source holds, for the "Found 312 mods in ..." line: the
/// entries of a manager's own mod list, else the archives and mod folders
/// directly inside `dir` (the full scan goes deeper).
fn count_items(dir: &Path) -> usize {
    if let Some(file) = layouts::find_arsenal_data(dir) {
        if let Ok(data) = layouts::load_arsenal(&file) {
            return data.mods.iter().filter(|m| m.path.is_dir()).count();
        }
    }
    quick_count(dir)
}

fn quick_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        let archive = ft.is_file() && is_archive_name(&entry.file_name().to_string_lossy());
        if archive || (ft.is_dir() && folder_is_mod(&path)) {
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    Archive,
    Folder,
}

/// Nexus Mods identity of an item, from its manifest or its (Nexus-style)
/// file/folder name. Never looked up online here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NexusRef {
    pub mod_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<i64>,
    /// The archive file name, when it's a Nexus download name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// Which of two downloads of the same file is newer (upload time, to
    /// the second or minute).
    #[serde(skip)]
    pub upload_order: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "Kind", rename_all = "PascalCase")]
pub enum ItemStatus {
    /// Not in DDMM yet: selected by default.
    New,
    /// Already in DDMM as `name`.
    Installed { name: String },
    /// Already in DDMM as `name`, but a different version of it.
    InstalledOtherVersion { name: String },
    /// The same archive/mod as another item in this list (`of` is its name).
    Duplicate { of: String },
    /// An older download of the same Nexus file as another item.
    OlderVersion { of: String },
    /// An archive/folder without any Helldivers 2 patch files or manifest.
    NotAMod,
    /// Couldn't be read (corrupt archive, unsafe paths, bad manifest...).
    Unreadable { reason: String },
}

impl ItemStatus {
    /// Checked in the preview by default.
    pub fn selected_by_default(&self) -> bool {
        matches!(self, ItemStatus::New)
    }

    /// Can't be imported at all (the checkbox is disabled). Everything
    /// else can still be picked by hand (e.g. something "already
    /// installed" only by its name).
    pub fn blocked(&self) -> bool {
        matches!(self, ItemStatus::Unreadable { .. } | ItemStatus::Duplicate { .. })
    }
}

/// Whether and where a mod sat in the source manager's active profile, and
/// with which options, so it lands in DDMM the same way.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProfileHint {
    pub enabled: bool,
    /// Position in the source's load order (0 = first).
    pub order: usize,
    /// Which options are on, and which sub-option each has chosen, when the
    /// source records that (see [`build_config`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggled: Option<Vec<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<Vec<usize>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScanItem {
    pub id: usize,
    pub kind: ItemKind,
    pub path: PathBuf,
    /// The name the mod will get (manifest name, else a cleaned-up file
    /// name).
    pub name: String,
    /// Bytes the mod takes once installed (unpacked).
    pub size: u64,
    /// Bytes of the archive/folder on disk.
    pub file_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nexus: Option<NexusRef>,
    pub status: ItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileHint>,
    /// Sources recorded by the source manager next to the mod (another
    /// DDMM/HD2MM `.hd2mm-origin.json`), carried over on import.
    #[serde(skip)]
    pub carried_sidecar: Option<OriginSidecar>,
    /// Filled lazily (only when two archives have the same size, or an
    /// installed mod was imported from an archive of this size).
    #[serde(skip)]
    pub sha256: Option<String>,
    /// Whether the manifest was generated by another manager (a `LOCAL`
    /// GUID), i.e. says nothing about the mod's identity across installs.
    #[serde(skip)]
    pub local_guid: bool,
    /// A manifest to write for a mod that ships none, built from what the
    /// source manager knows about it (its name and options).
    #[serde(skip)]
    pub manifest_override: Option<Manifest>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScanResult {
    /// The folder scanned (`None` for a list of picked files).
    pub root: Option<PathBuf>,
    pub items: Vec<ScanItem>,
    /// The source manager's active profile, if its profile file was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// The scan stopped at [`MAX_SCAN_ITEMS`].
    pub truncated: bool,
}

/// Scan progress: `done` of `total` items read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScanProgress {
    pub done: usize,
    pub total: usize,
}

/// A candidate found while walking the source.
#[derive(Debug, Clone)]
struct Candidate {
    kind: ItemKind,
    path: PathBuf,
}

fn is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if crate::commands::handoff::IGNORED_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return false;
    }
    Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ARCHIVE_EXTENSIONS.contains(&e))
}

/// Refuse a source that is, contains, or is inside DDMM's data folder:
/// importing from there would copy DDMM's own mods into themselves.
pub fn ensure_source_allowed(source: &Path, base_path: &Path) -> anyhow::Result<()> {
    match path_overlap(source, base_path)? {
        None => Ok(()),
        Some(PathOverlap::Same) | Some(PathOverlap::FirstInsideSecond) => anyhow::bail!(
            "{} is inside DDMM's own data folder ({}). Those mods are already DDMM's; pick the folder \
             where the other mod manager or your downloads keep them.",
            source.display(),
            base_path.display()
        ),
        Some(PathOverlap::SecondInsideFirst) => anyhow::bail!(
            "DDMM's own data folder ({}) is inside {}, so importing from there would copy DDMM's mods \
             into themselves. Pick the folder that holds just the mods you want to import.",
            base_path.display(),
            source.display()
        ),
    }
}

/// Walk `root` for archives and mod folders (see the module docs).
/// Symlinks are never followed. A folder that is a mod isn't searched any
/// further (its variant folders are part of it).
fn collect_candidates(root: &Path, cancel: &AtomicBool) -> (Vec<Candidate>, bool) {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut listing: Vec<_> = entries.flatten().collect();
        listing.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for entry in listing {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_file() && is_archive_name(&name) {
                out.push(Candidate { kind: ItemKind::Archive, path });
            } else if ft.is_dir() {
                if SKIPPED_DIR_NAMES.iter().any(|s| s.eq_ignore_ascii_case(&name)) || name.starts_with('.') {
                    continue;
                }
                if folder_is_mod(&path) {
                    out.push(Candidate { kind: ItemKind::Folder, path });
                } else if depth + 1 < MAX_SCAN_DEPTH {
                    subdirs.push(path);
                }
            }
            if out.len() >= MAX_SCAN_ITEMS {
                return (out, true);
            }
        }
        // Depth-first, in name order.
        for sub in subdirs.into_iter().rev() {
            stack.push((sub, depth + 1));
        }
    }
    (out, false)
}

/// A folder is a mod if it has a manifest.json, or Helldivers 2 patch files
/// (up to [`PATCH_SEARCH_DEPTH`] levels down) and no subfolder that is a
/// mod with its own manifest.json (that makes it a folder *of* mods).
fn folder_is_mod(dir: &Path) -> bool {
    if has_manifest_sync(dir) {
        return true;
    }
    let holds_mods = std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) && has_manifest_sync(&e.path())
            })
        })
        .unwrap_or(false);
    !holds_mods && has_patch_files(dir, 0)
}

fn has_manifest_sync(dir: &Path) -> bool {
    find_manifest_sync(dir).is_some()
}

fn find_manifest_sync(dir: &Path) -> Option<PathBuf> {
    let exact = dir.join(crate::commands::mods::MANIFEST_FILE);
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let is_file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        (is_file && e.file_name().to_string_lossy().eq_ignore_ascii_case(crate::commands::mods::MANIFEST_FILE))
            .then(|| e.path())
    })
}

fn has_patch_files(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_file() && is_patch_filename(&entry.file_name().to_string_lossy()) {
            return true;
        }
        if ft.is_dir() && !ft.is_symlink() {
            subdirs.push(entry.path());
        }
    }
    depth < PATCH_SEARCH_DEPTH && subdirs.iter().any(|d| has_patch_files(d, depth + 1))
}

/// Bytes in a folder tree, not following symlinks.
fn tree_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if ft.is_dir() && depth < crate::utils::MAX_COPY_DEPTH {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    total
}

/// The Nexus identity of an archive or folder name, if it follows one of
/// Nexus Mods' download naming schemes (see
/// [`sources::parse_nexus_archive_name`]; an unpacked folder of such an
/// archive keeps the name minus extension).
pub fn nexus_ref_from_name(file_name: &str, is_archive: bool) -> Option<NexusRef> {
    let as_archive = if is_archive { file_name.to_string() } else { format!("{file_name}.zip") };
    let parsed = sources::parse_nexus_archive_name(&as_archive)?;
    Some(NexusRef {
        mod_id: parsed.mod_id,
        file_id: None,
        version: Some(parsed.version),
        uploaded_at: parsed.uploaded_at,
        file_name: is_archive.then(|| file_name.to_string()),
        upload_order: parsed.upload_order,
    })
}

/// A readable mod name from an archive/folder name: the extension and a
/// Nexus download suffix (`-1234-1-0-1712345678`, ` 1234 1.0 2026-...Z slug`,
/// ` (1)`) removed.
pub fn display_name_from_file(file_name: &str, is_archive: bool) -> String {
    let as_archive = if is_archive { file_name.to_string() } else { format!("{file_name}.zip") };
    if let Some(parsed) = sources::parse_nexus_archive_name(&as_archive) {
        if !parsed.name.trim().is_empty() {
            return parsed.name.trim().to_string();
        }
    }
    let stem = if is_archive {
        let lower = file_name.to_ascii_lowercase();
        ARCHIVE_EXTENSIONS
            .iter()
            .find_map(|e| lower.strip_suffix(&format!(".{e}")).map(|s| file_name[..s.len()].to_string()))
            .unwrap_or_else(|| file_name.to_string())
    } else {
        file_name.to_string()
    };
    let cleaned = stem.trim().to_string();
    if cleaned.is_empty() { file_name.to_string() } else { cleaned }
}

/// Nexus id declared by a manifest (legacy `NexusData` or a `Sources`
/// entry), if any.
fn manifest_nexus(manifest: &Manifest) -> Option<NexusRef> {
    type Declared<'a> = (Option<&'a Vec<Source>>, Option<(u64, Option<String>)>);
    let (sources, nexus_data): Declared = match manifest {
        Manifest::Legacy(_) => (None, None),
        Manifest::V1(m) => (m.sources.as_ref(), m.nexus_data.as_ref().map(|n| (n.mod_id, Some(n.version.clone())))),
        Manifest::V2(m) => (m.sources.as_ref(), m.nexus_data.as_ref().map(|n| (n.mod_id, None))),
    };
    if let Some(s) = sources.into_iter().flatten().find(|s| s.provider.eq_ignore_ascii_case("nexus") && s.id.is_some()) {
        return Some(NexusRef { mod_id: s.id.clone().unwrap_or_default(), version: s.version.clone(), ..Default::default() });
    }
    nexus_data.map(|(id, version)| NexusRef { mod_id: id.to_string(), version, ..Default::default() })
}

fn guid_is_local(guid: &Uuid) -> bool {
    guid.as_bytes().get(..5) == Some(b"LOCAL")
}

/// Read one candidate without extracting or changing anything.
fn inspect(id: usize, candidate: &Candidate) -> ScanItem {
    let file_name = candidate.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let is_archive = candidate.kind == ItemKind::Archive;
    let mut item = ScanItem {
        id,
        kind: candidate.kind,
        path: candidate.path.clone(),
        name: display_name_from_file(&file_name, is_archive),
        size: 0,
        file_size: 0,
        guid: None,
        nexus: nexus_ref_from_name(&file_name, is_archive),
        status: ItemStatus::New,
        profile: None,
        carried_sidecar: None,
        sha256: None,
        local_guid: false,
        manifest_override: None,
    };

    let result = match candidate.kind {
        ItemKind::Archive => inspect_archive(&mut item),
        ItemKind::Folder => inspect_folder(&mut item),
    };
    if let Err(e) = result {
        item.status = ItemStatus::Unreadable { reason: format!("{e:#}") };
    }
    item
}

fn apply_manifest(item: &mut ScanItem, manifest: &Manifest) {
    let (guid, name) = match manifest {
        Manifest::Legacy(m) => (m.guid, m.name.clone()),
        Manifest::V1(m) => (m.guid, m.name.clone()),
        Manifest::V2(m) => (m.guid, m.name.clone()),
    };
    item.guid = Some(guid);
    item.local_guid = guid_is_local(&guid);
    if !name.trim().is_empty() {
        item.name = name.trim().to_string();
    }
    if let Some(n) = manifest_nexus(manifest) {
        // The file name knows the exact installed file; the manifest only
        // the mod. Keep the richer one when they agree.
        match &item.nexus {
            Some(existing) if existing.mod_id == n.mod_id => {}
            _ => item.nexus = Some(n),
        }
    }
}

fn inspect_archive(item: &mut ScanItem) -> anyhow::Result<()> {
    item.file_size = std::fs::metadata(&item.path)?.len();
    let mut archive = Archive::open(&item.path).map_err(|e| anyhow::anyhow!("can't open the archive: {e:#}"))?;
    archive.validate_entries()?;
    let mut unpacked = 0u64;
    let mut has_patch = false;
    for entry in archive.iter()? {
        let entry = entry.map_err(|e| anyhow::anyhow!("the archive is damaged: {e:#}"))?;
        if entry.is_directory() {
            continue;
        }
        unpacked = unpacked.saturating_add(entry.size());
        if let Some(name) = entry.path().file_name().and_then(|n| n.to_str()) {
            has_patch |= is_patch_filename(name);
        }
    }
    item.size = unpacked;
    let manifest_entry = archive.find_root_file_ci(crate::commands::mods::MANIFEST_FILE)?;
    if let Some(entry) = &manifest_entry {
        let data = archive.read_path(entry)?;
        let manifest = Manifest::parse(&data, "manifest.json")?;
        apply_manifest(item, &manifest);
        // An author manifest's GUID is the mod's identity; one generated by
        // a manager is only meaningful together with its folder.
    }
    if !has_patch && manifest_entry.is_none() {
        item.status = ItemStatus::NotAMod;
    }
    Ok(())
}

fn inspect_folder(item: &mut ScanItem) -> anyhow::Result<()> {
    item.size = tree_size(&item.path);
    item.file_size = item.size;
    if let Some(manifest_file) = find_manifest_sync(&item.path) {
        let data = std::fs::read(&manifest_file)?;
        let manifest = Manifest::parse(&data, "manifest.json")?;
        apply_manifest(item, &manifest);
    }
    let sidecar_path = item.path.join(ORIGIN_SIDECAR_FILE);
    if let Ok(data) = std::fs::read(&sidecar_path) {
        if let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&data) {
            if item.nexus.is_none() {
                if let Some(s) = sidecar.sources.iter().find(|s| s.provider.eq_ignore_ascii_case("nexus") && s.id.is_some()) {
                    let file = sidecar.installed_files.iter().find(|f| f.provider.eq_ignore_ascii_case("nexus"));
                    item.nexus = Some(NexusRef {
                        mod_id: s.id.clone().unwrap_or_default(),
                        file_id: file.and_then(|f| f.file_id.clone()),
                        version: s.version.clone(),
                        uploaded_at: file.and_then(|f| f.uploaded_at),
                        file_name: file.and_then(|f| f.file_name.clone()),
                        upload_order: file.and_then(|f| f.uploaded_at).unwrap_or(0),
                    });
                }
            }
            item.carried_sidecar = Some(sidecar);
        }
    }
    Ok(())
}

/// An installed mod's Nexus page and file.
#[derive(Debug)]
struct InstalledNexus {
    mod_id: String,
    version: Option<String>,
    file_name: Option<String>,
    file_id: Option<String>,
    name: String,
}

/// What DDMM already has, for "already installed" checks.
#[derive(Debug, Default)]
pub struct InstalledIndex {
    guids: HashMap<Uuid, String>,
    nexus: Vec<InstalledNexus>,
    fingerprints: Vec<(ArchiveFingerprint, String)>,
    /// Lower-cased archive file names DDMM installed from, and mod folder
    /// names (a plain Add File names the folder after the archive).
    file_names: HashMap<String, String>,
}

impl InstalledIndex {
    pub async fn build(mods: &[Mod]) -> Self {
        let mut index = InstalledIndex::default();
        for m in mods {
            let name = m.name().to_string();
            index.guids.insert(m.guid(), name.clone());
            if let Some(dir) = m.directory.file_name() {
                index.file_names.insert(dir.to_string_lossy().to_lowercase(), name.clone());
            }
            let sidecar = sources::load_origin_sidecar(&m.directory).await;
            let files = sidecar.as_ref().map(|s| s.installed_files.clone()).unwrap_or_default();
            for f in &files {
                if let Some(fname) = &f.file_name {
                    index.file_names.insert(fname.to_lowercase(), name.clone());
                }
            }
            if let Some(fp) = sidecar.as_ref().and_then(|s| s.imported_archive.clone()) {
                if let Some(fname) = &fp.file_name {
                    index.file_names.insert(fname.to_lowercase(), name.clone());
                }
                index.fingerprints.push((fp, name.clone()));
            }
            let nexus_file = files.iter().find(|f| f.provider.eq_ignore_ascii_case("nexus"));
            let sidecar_versions: HashMap<String, Option<String>> = sidecar
                .as_ref()
                .map(|s| {
                    s.sources
                        .iter()
                        .filter(|s| s.provider.eq_ignore_ascii_case("nexus"))
                        .filter_map(|s| s.id.clone().map(|id| (id, s.version.clone())))
                        .collect()
                })
                .unwrap_or_default();
            for source in m.sources.iter().filter(|s| s.provider.eq_ignore_ascii_case("nexus")) {
                if let Some(id) = sources::resolved_source_id(source) {
                    let version = sidecar_versions.get(&id).cloned().flatten().or_else(|| source.version.clone());
                    index.nexus.push(InstalledNexus {
                        mod_id: id,
                        version,
                        file_name: nexus_file.and_then(|f| f.file_name.clone()),
                        file_id: nexus_file.and_then(|f| f.file_id.clone()),
                        name: name.clone(),
                    });
                }
            }
        }
        index
    }

    fn installed_sizes(&self) -> HashSet<u64> {
        self.fingerprints.iter().map(|(f, _)| f.size).collect()
    }
}

/// SHA-256 of a file, hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn versions_equal(a: &str, b: &str) -> bool {
    crate::providers::compare_versions(a, b) == crate::providers::VersionRelation::Same
}

/// Decide each item's status: already installed, duplicate, older
/// version. Items that are unreadable or not mods keep that status.
pub fn resolve_statuses(items: &mut [ScanItem], installed: &InstalledIndex) {
    // Hash archives only where a hash can actually decide something: two
    // archives of the same size, or one the size of an archive an
    // installed mod was imported from.
    let mut size_counts: HashMap<u64, usize> = HashMap::new();
    for item in items.iter().filter(|i| i.kind == ItemKind::Archive && i.status == ItemStatus::New) {
        *size_counts.entry(item.file_size).or_default() += 1;
    }
    let installed_sizes = installed.installed_sizes();
    for item in items.iter_mut().filter(|i| i.kind == ItemKind::Archive && i.status == ItemStatus::New) {
        if size_counts.get(&item.file_size).copied().unwrap_or(0) > 1 || installed_sizes.contains(&item.file_size) {
            item.sha256 = sha256_file(&item.path).ok();
        }
    }

    // Against what's installed.
    for item in items.iter_mut() {
        if item.status != ItemStatus::New {
            continue;
        }
        if let Some(status) = installed_status(item, installed) {
            item.status = status;
        }
    }

    // Within this list: identical archives, same manifest GUID, same
    // Nexus file downloaded more than once.
    // Originals before copies: `Mod.zip` before `Mod (1).zip`, then the
    // shorter path.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (looks_like_a_copy(&items[i].path), items[i].path.as_os_str().len(), i));
    let mut by_hash: HashMap<(u64, String), String> = HashMap::new();
    let mut by_guid: HashMap<Uuid, String> = HashMap::new();
    for i in order {
        let item = &mut items[i];
        if !matches!(item.status, ItemStatus::New | ItemStatus::InstalledOtherVersion { .. }) {
            continue;
        }
        if let Some(sha) = &item.sha256 {
            let key = (item.file_size, sha.clone());
            if let Some(of) = by_hash.get(&key) {
                item.status = ItemStatus::Duplicate { of: of.clone() };
                continue;
            }
            by_hash.insert(key, item.name.clone());
        }
        if let Some(guid) = item.guid.filter(|_| !item.local_guid || item.kind == ItemKind::Folder) {
            if let Some(of) = by_guid.get(&guid) {
                item.status = ItemStatus::Duplicate { of: of.clone() };
                continue;
            }
            by_guid.insert(guid, item.name.clone());
        }
    }

    // Several downloads of the same Nexus file (same mod id, same file
    // name apart from version/date): keep the newest selected.
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if !matches!(item.status, ItemStatus::New) {
            continue;
        }
        let Some(nexus) = item.nexus.as_ref().filter(|n| n.upload_order > 0) else { continue };
        let shape = crate::providers::file_shape(&item.name);
        groups.entry((nexus.mod_id.clone(), shape)).or_default().push(i);
    }
    for indices in groups.values().filter(|g| g.len() > 1) {
        let newest = *indices
            .iter()
            .max_by_key(|&&i| items[i].nexus.as_ref().map(|n| n.upload_order).unwrap_or(0))
            .unwrap();
        let newest_name = items[newest].name.clone();
        for &i in indices {
            if i != newest {
                items[i].status = ItemStatus::OlderVersion { of: newest_name.clone() };
            }
        }
    }
}

/// `Name (1).zip`, `Name - Copy.zip`: what browsers and Explorer call a
/// second copy of a file.
fn looks_like_a_copy(path: &Path) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)(?: \(\d+\)| - copy(?: \(\d+\))?)$").unwrap());
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    re.is_match(&stem)
}

fn installed_status(item: &ScanItem, installed: &InstalledIndex) -> Option<ItemStatus> {
    // Same manifest GUID. A manager-generated (`LOCAL...`) GUID only counts
    // for a folder, which carries its manifest (and so its GUID) along.
    if let Some(guid) = item.guid {
        if !item.local_guid || item.kind == ItemKind::Folder {
            if let Some(name) = installed.guids.get(&guid) {
                return Some(ItemStatus::Installed { name: name.clone() });
            }
        }
    }
    // The very same archive.
    if let Some(sha) = &item.sha256 {
        if let Some((_, name)) = installed.fingerprints.iter().find(|(f, _)| f.size == item.file_size && &f.sha256 == sha) {
            return Some(ItemStatus::Installed { name: name.clone() });
        }
    }
    // Installed from a file of the same name before (or, for a folder, a
    // mod folder of the same name).
    let file_name = item.path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem = item.path.file_stem().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let by_name = match item.kind {
        ItemKind::Archive => installed.file_names.get(&file_name).or_else(|| installed.file_names.get(&stem)),
        ItemKind::Folder => installed.file_names.get(&file_name),
    };
    if let Some(name) = by_name {
        return Some(ItemStatus::Installed { name: name.clone() });
    }
    // Same Nexus mod.
    if let Some(nexus) = &item.nexus {
        let same_mod: Vec<&InstalledNexus> = installed.nexus.iter().filter(|n| n.mod_id == nexus.mod_id).collect();
        if !same_mod.is_empty() {
            let same_file = same_mod.iter().find(|i| {
                (nexus.file_id.is_some() && i.file_id == nexus.file_id)
                    || (nexus.file_name.is_some()
                        && i.file_name.as_deref().map(str::to_lowercase) == nexus.file_name.as_deref().map(str::to_lowercase))
                    || matches!((&i.version, &nexus.version), (Some(a), Some(b)) if versions_equal(a, b))
            });
            if let Some(i) = same_file {
                return Some(ItemStatus::Installed { name: i.name.clone() });
            }
            // A Nexus page can hold several different files (a main file
            // and optional variants): only call it "another version" when
            // the names match up too.
            let shape = crate::providers::file_shape(&item.name);
            if let Some(i) = same_mod.iter().find(|i| crate::providers::file_shape(&i.name) == shape) {
                return Some(ItemStatus::InstalledOtherVersion { name: i.name.clone() });
            }
        }
    }
    None
}

/// Scan a folder (see the module docs). Blocking; call from a blocking
/// thread. `progress` gets called after every item.
///
/// A folder of another mod manager's is read with that manager's own
/// records where it has any (see [`layouts`]): its mod list, names, Nexus
/// ids, on/off state, load order and options.
pub fn scan_folder(
    root: &Path,
    installed: &InstalledIndex,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> anyhow::Result<ScanResult> {
    if !root.is_dir() {
        anyhow::bail!("{} isn't a folder (or can't be opened)", root.display());
    }
    let arsenal = layouts::find_arsenal_data(root).and_then(|file| match layouts::load_arsenal(&file) {
        Ok(data) => Some((file, data)),
        Err(e) => {
            log::warn!("Import: couldn't read {file:?} ({e:#}); scanning the folder instead.");
            None
        }
    });

    let mut result = if let Some((file, data)) = arsenal {
        let data_dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let entries: Vec<(Candidate, layouts::ArsenalMod)> = data
            .mods
            .into_iter()
            .map(|m| {
                let path = if m.path.is_relative() { data_dir.join(&m.path) } else { m.path.clone() };
                (Candidate { kind: ItemKind::Folder, path }, m)
            })
            .filter(|(c, _)| c.path.is_dir())
            .collect();
        let candidates: Vec<Candidate> = entries.iter().map(|(c, _)| c.clone()).collect();
        let mut items = inspect_all(&candidates, cancel, &mut progress);
        for (item, (_, m)) in items.iter_mut().zip(&entries) {
            apply_arsenal(item, m);
        }
        ScanResult { root: None, items, profile_name: data.profile_name, truncated: false }
    } else {
        let (candidates, truncated) = collect_candidates(root, cancel);
        let mut items = inspect_all(&candidates, cancel, &mut progress);
        let mut profile_name = None;
        if let Some((name, hints)) = layouts::load_guid_hints(root) {
            for item in items.iter_mut() {
                if let Some(hint) = item.guid.and_then(|g| hints.get(&g)) {
                    item.profile = Some(hint.clone());
                }
            }
            profile_name = name;
        }
        if let Some(hints) = layouts::load_patch_order(root) {
            for item in items.iter_mut().filter(|i| i.kind == ItemKind::Folder && i.profile.is_none()) {
                let folder = item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(hint) = hints.get(&folder) {
                    item.profile = Some(hint.clone());
                }
            }
        }
        ScanResult { root: None, items, profile_name, truncated }
    };
    result.root = Some(root.to_path_buf());
    resolve_statuses(&mut result.items, installed);
    Ok(result)
}

/// Use what Arsenal's library knows about a mod folder.
fn apply_arsenal(item: &mut ScanItem, m: &layouts::ArsenalMod) {
    if m.nexus.is_some() {
        item.nexus = m.nexus.clone();
    }
    item.profile = m.profile.clone();
    // A folder with the author's manifest.json keeps it; for the rest,
    // Arsenal's name, id and options become the manifest.
    if item.guid.is_none() && !matches!(item.status, ItemStatus::Unreadable { .. }) {
        if !m.label.trim().is_empty() {
            item.name = m.label.trim().to_string();
        }
        if let Ok(guid) = Uuid::parse_str(&m.uuid) {
            item.guid = Some(guid);
        }
        item.manifest_override = layouts::arsenal_manifest(m);
    }
}

/// Scan an explicit list of archives and folders (Add with many files, or
/// many dropped at once). Each folder is one mod.
pub fn scan_paths(
    paths: &[PathBuf],
    installed: &InstalledIndex,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> ScanResult {
    let candidates: Vec<Candidate> = paths
        .iter()
        .map(|p| Candidate { kind: if p.is_dir() { ItemKind::Folder } else { ItemKind::Archive }, path: p.clone() })
        .collect();
    let mut items = inspect_all(&candidates, cancel, &mut progress);
    resolve_statuses(&mut items, installed);
    ScanResult { root: None, items, profile_name: None, truncated: false }
}

fn inspect_all(candidates: &[Candidate], cancel: &AtomicBool, progress: &mut impl FnMut(ScanProgress)) -> Vec<ScanItem> {
    let total = candidates.len();
    let mut items = Vec::with_capacity(total);
    for (id, candidate) in candidates.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        items.push(inspect(id, candidate));
        progress(ScanProgress { done: id + 1, total });
    }
    items
}

// ---------------------------------------------------------------------------
// Importing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportProgress {
    pub done: usize,
    pub total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// The mod being imported now.
    pub current: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportedMod {
    pub id: usize,
    pub name: String,
    pub guid: Uuid,
    /// From the source manager's profile, when it had one: whether the mod
    /// was on, where in the load order it was, and its profile entry
    /// (options included) ready to add to a DDMM profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Config>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportProblem {
    pub id: usize,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportReport {
    pub imported: Vec<ImportedMod>,
    pub failed: Vec<ImportProblem>,
    /// Chosen, but not started because the import was cancelled.
    pub not_started: usize,
    pub cancelled: bool,
    /// The mod that was being imported when Cancel was pressed; removed
    /// again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolled_back: Option<String>,
}

/// Refuse an import that wouldn't fit: `needed` bytes plus a margin
/// against what's `free` (when the OS could tell).
pub fn check_free_space(needed: u64, free: Option<u64>) -> anyhow::Result<()> {
    if let Some(free) = free {
        if free < needed.saturating_add(FREE_SPACE_MARGIN) {
            anyhow::bail!(
                "Not enough free space: the selected mods need about {} (plus {} to spare) and only {} is free \
                 on the drive with DDMM's data folder. Free up some space or select fewer mods.",
                crate::data_move::human_bytes(needed),
                crate::data_move::human_bytes(FREE_SPACE_MARGIN),
                crate::data_move::human_bytes(free)
            );
        }
    }
    Ok(())
}

/// A folder name that's valid on every OS DDMM runs on, derived from `name`.
pub fn safe_dir_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    let out = out.trim_start().to_string();
    let upper = out.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    let reserved = matches!(stem, "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    let mut out = if out.is_empty() || reserved { format!("mod {out}").trim().to_string() } else { out };
    // Keep paths comfortably short on Windows.
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect::<String>().trim_end().to_string();
    }
    out
}

/// `mods/<name>`, or `mods/<name> (2)`, ... -- the first that's free.
fn unique_dir_name(mods_root: &Path, name: &str, taken: &HashSet<String>) -> String {
    let base = safe_dir_name(name);
    let free = |candidate: &str| {
        !taken.contains(&candidate.to_lowercase()) && std::fs::symlink_metadata(mods_root.join(candidate)).is_err()
    };
    if free(&base) {
        return base;
    }
    (2..).map(|n| format!("{base} ({n})")).find(|c| free(c)).unwrap()
}

/// The sidecar sources/files to record for an imported mod.
fn sidecar_for(item: &ScanItem, installed: &Mod, fingerprint: Option<ArchiveFingerprint>) -> OriginSidecar {
    let mut sidecar = item.carried_sidecar.clone().unwrap_or(OriginSidecar {
        sources: Vec::new(),
        installed_at: 0,
        installed_files: Vec::new(),
        skipped_versions: Vec::new(),
        imported_archive: None,
    });
    sidecar.installed_at =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Some(nexus) = &item.nexus {
        // Record it as an install source unless the mod's own manifest
        // already names the same Nexus page (it would show twice); the
        // exact file is recorded either way.
        let declared = installed.sources.iter().any(|s| {
            s.provider.eq_ignore_ascii_case("nexus")
                && s.origin == sources::SourceOrigin::Manifest
                && sources::resolved_source_id(s).as_deref() == Some(nexus.mod_id.as_str())
        });
        let have = sidecar
            .sources
            .iter()
            .any(|s| s.provider.eq_ignore_ascii_case("nexus") && s.id.as_deref() == Some(nexus.mod_id.as_str()));
        if !have && !declared {
            sidecar.sources.push(Source {
                provider: "nexus".into(),
                id: Some(nexus.mod_id.clone()),
                url: None,
                version: nexus.version.clone(),
            });
        }
        let has_file = sidecar.installed_files.iter().any(|f| f.provider.eq_ignore_ascii_case("nexus"));
        if !has_file && (nexus.file_id.is_some() || nexus.file_name.is_some() || nexus.uploaded_at.is_some()) {
            sidecar.installed_files.push(InstalledFile {
                provider: "nexus".into(),
                file_id: nexus.file_id.clone(),
                file_name: nexus.file_name.clone(),
                label: None,
                uploaded_at: nexus.uploaded_at,
            });
        }
    }
    if fingerprint.is_some() {
        sidecar.imported_archive = fingerprint;
    }
    sidecar
}

/// Import `items` (already chosen by the user) into DDMM, one at a time.
///
/// The caller holds the data-folder lock ([`AppState::data_op`]) for the
/// whole run. `cancel` is checked before each item and after it: an item
/// that was in flight when Cancel was pressed is removed again, everything
/// finished before stays. A failing item is recorded and skipped.
pub async fn run_import(
    state: &AppState,
    items: Vec<ScanItem>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ImportProgress),
) -> ImportReport {
    let mods_root = state.base_path.join(MODS_DIRECTORY);
    let _ = tokio::fs::create_dir_all(&mods_root).await;
    let total = items.len();
    let bytes_total: u64 = items.iter().map(|i| i.size).sum();
    let mut bytes_done = 0u64;
    let mut report = ImportReport::default();
    let mut taken: HashSet<String> = HashSet::new();

    for (index, item) in items.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            report.cancelled = true;
            report.not_started = total - index;
            break;
        }
        progress(ImportProgress { done: index, total, bytes_done, bytes_total, current: Some(item.name.clone()) });

        match import_one(state, &mods_root, item, &mut taken).await {
            Ok((installed, warning)) => {
                if cancel.load(Ordering::SeqCst) {
                    // Pressed while this one was being copied: undo it.
                    remove_imported(state, &installed).await;
                    report.cancelled = true;
                    report.rolled_back = Some(item.name.clone());
                    report.not_started = total - index - 1;
                    break;
                }
                report.imported.push(ImportedMod {
                    id: item.id,
                    name: installed.name().to_string(),
                    guid: installed.guid(),
                    enabled: item.profile.as_ref().map(|h| h.enabled),
                    order: item.profile.as_ref().map(|h| h.order),
                    config: item.profile.as_ref().map(|h| build_config(&installed, h)),
                    warning,
                });
            }
            Err(e) => {
                log::warn!("Import of {:?} failed: {e:#}", item.path);
                report.failed.push(ImportProblem { id: item.id, name: item.name.clone(), reason: format!("{e:#}") });
                if cancel.load(Ordering::SeqCst) {
                    report.cancelled = true;
                    report.not_started = total - index - 1;
                    break;
                }
            }
        }
        bytes_done = bytes_done.saturating_add(item.size);
    }
    progress(ImportProgress {
        done: report.imported.len() + report.failed.len(),
        total,
        bytes_done,
        bytes_total,
        current: None,
    });
    report
}

/// For a mod that shipped no manifest.json (so DDMM generated a `LOCAL`
/// one), use what the source manager knew instead: its options as a v1
/// manifest, or at least its ID for the generated one -- so the mod keeps
/// its identity (a second import recognizes it) and the source's profile
/// settings fit.
async fn apply_manifest_override(state: &AppState, item: &ScanItem, installed: &mut Mod) -> anyhow::Result<()> {
    let generated = matches!(&installed.manifest, Manifest::Legacy(m) if guid_is_local(&m.guid));
    if !generated {
        return Ok(());
    }
    let old_guid = installed.guid();
    let new_manifest = match (&item.manifest_override, item.guid) {
        (Some(manifest), _) => manifest.clone(),
        (None, Some(guid)) if !guid_is_local(&guid) => {
            let Manifest::Legacy(mut m) = installed.manifest.clone() else { return Ok(()) };
            m.guid = guid;
            Manifest::Legacy(m)
        }
        _ => return Ok(()),
    };
    let new_guid = match &new_manifest {
        Manifest::Legacy(m) => m.guid,
        Manifest::V1(m) => m.guid,
        Manifest::V2(m) => m.guid,
    };
    let mut guard = state.mods.lock().await;
    let mods = guard.as_mut().ok_or_else(|| anyhow::anyhow!("mods not read"))?;
    if new_guid != old_guid && mods.iter().any(|m| m.guid() == new_guid) {
        anyhow::bail!("a mod with GUID {new_guid} is already installed; keeping the generated one");
    }
    let data = serde_json::to_vec_pretty(&new_manifest)?;
    tokio::fs::write(installed.directory.join(crate::commands::mods::MANIFEST_FILE), data).await?;
    installed.manifest = new_manifest;
    if let Err(e) = installed.normalize_paths().await {
        log::warn!("Path normalization failed for {:?}: {e:#}", installed.directory);
    }
    if let Some(entry) = mods.iter_mut().find(|m| m.guid() == old_guid && m.directory == installed.directory) {
        *entry = installed.clone();
    }
    Ok(())
}

/// A profile entry for `installed` from the source's state, shaped for its
/// manifest (option lists that don't fit it fall back to DDMM's defaults:
/// every option on, first choice).
pub fn build_config(installed: &Mod, hint: &ProfileHint) -> Config {
    let guid = installed.guid();
    match &installed.manifest {
        Manifest::Legacy(m) => {
            let count = m.options.as_ref().map(Vec::len).unwrap_or(0);
            let selected = hint.selected.as_ref().and_then(|s| s.first().copied()).filter(|&i| i < count.max(1)).unwrap_or(0);
            Config::Legacy { guid, enabled: hint.enabled, selected }
        }
        Manifest::V1(m) => {
            let subs: Vec<usize> = m
                .options
                .iter()
                .flatten()
                .map(|o| o.sub_options.as_ref().map(Vec::len).unwrap_or(0))
                .collect();
            let (toggled, selected) = fit_options(hint, &subs);
            Config::V1 { guid, enabled: hint.enabled, toggled, selected }
        }
        Manifest::V2(m) => {
            let subs: Vec<usize> = m
                .options
                .iter()
                .flatten()
                .map(|o| o.sub_options.as_ref().map(Vec::len).unwrap_or(0))
                .collect();
            let (toggled, selected) = fit_options(hint, &subs);
            Config::V2 { guid, enabled: hint.enabled, toggled, selected }
        }
    }
}

fn fit_options(hint: &ProfileHint, sub_counts: &[usize]) -> (Vec<bool>, Vec<usize>) {
    let n = sub_counts.len();
    let toggled = hint.toggled.clone().filter(|t| t.len() == n).unwrap_or_else(|| vec![true; n]);
    let selected = hint
        .selected
        .clone()
        .filter(|s| s.len() == n && s.iter().zip(sub_counts).all(|(&i, &count)| i < count.max(1)))
        .unwrap_or_else(|| vec![0; n]);
    (toggled, selected)
}

async fn remove_imported(state: &AppState, installed: &Mod) {
    let mut guard = state.mods.lock().await;
    if let Some(mods) = guard.as_mut() {
        mods.retain(|m| m.guid() != installed.guid() || m.directory != installed.directory);
    }
    if let Err(e) = tokio::fs::remove_dir_all(&installed.directory).await {
        log::error!("Couldn't remove the cancelled import {:?}: {e}", installed.directory);
    }
}

async fn import_one(
    state: &AppState,
    mods_root: &Path,
    item: &ScanItem,
    taken: &mut HashSet<String>,
) -> anyhow::Result<(Mod, Option<String>)> {
    ensure_source_allowed(&item.path, &state.base_path)?;

    let fingerprint = if item.kind == ItemKind::Archive {
        let path = item.path.clone();
        let known = item.sha256.clone();
        let sha = match known {
            Some(s) => s,
            None => tokio::task::spawn_blocking(move || sha256_file(&path)).await??,
        };
        Some(ArchiveFingerprint {
            size: item.file_size,
            sha256: sha,
            file_name: item.path.file_name().map(|n| n.to_string_lossy().into_owned()),
        })
    } else {
        None
    };

    let dir_name = unique_dir_name(mods_root, &item.name, taken);
    taken.insert(dir_name.to_lowercase());

    let (mut installed, warning) = {
        let mut guard = state.mods.lock().await;
        let mods = guard.as_mut().ok_or_else(|| anyhow::anyhow!("mods not read"))?;
        let result = match item.kind {
            ItemKind::Archive => install_from_archive_as(&state.base_path, mods, &item.path, &dir_name).await,
            ItemKind::Folder => install_from_folder_as(&state.base_path, mods, &item.path, Some(&dir_name)).await,
        };
        result.map_err(|e| anyhow::anyhow!("{e}"))?
    };

    if let Err(e) = apply_manifest_override(state, item, &mut installed).await {
        // The mod is installed and works with the generated manifest;
        // only the source manager's option names are missing.
        log::warn!("Couldn't write the imported manifest for {:?}: {e:#}", installed.directory);
    }

    let sidecar = sidecar_for(item, &installed, fingerprint);
    if let Err(e) = sources::save_origin_sidecar(&installed.directory, &sidecar).await {
        log::error!("Couldn't record where {:?} was imported from: {e:#}", installed.directory);
    }
    installed.resolve_sources().await;
    {
        let mut guard = state.mods.lock().await;
        if let Some(mods) = guard.as_mut() {
            if let Some(existing) = mods.iter_mut().find(|m| m.guid() == installed.guid() && m.directory == installed.directory) {
                *existing = installed.clone();
            }
        }
    }
    Ok((installed, warning))
}

#[cfg(test)]
mod tests;
