use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}, sync::OnceLock};

use anyhow_tauri::{IntoTAResult, TAResult};
use regex::Regex;
use tauri::State;

use crate::{AppState, commands::settings::do_load_settings, models::{manifest::Manifest, profile::Config, settings::Settings, Mod}, utils::is_patch_filename};

pub mod mods;
pub mod profiles;
pub mod settings;
pub mod handoff;
pub mod updates;
pub mod nexus;
pub mod bridge;
pub mod data_folder;
pub mod import;

static INDEX_REGEX: OnceLock<Regex> = OnceLock::new();

struct PatchFileTriplet {
    patch: Option<PathBuf>,
    gpu_resources: Option<PathBuf>,
    stream: Option<PathBuf>,
}

/// `relative` (from a mod's manifest) resolved under the mod directory
/// `base`, tolerating Windows-style backslash separators and wrong casing --
/// see [`crate::utils::fix_path_casing`].
async fn resolve_mod_path(base: &Path, relative: &Path) -> PathBuf {
    match crate::utils::fix_path_casing(base, relative).await {
        Ok(fixed) => base.join(fixed),
        Err(_) => base.join(relative),
    }
}

async fn get_patch_files_from_dir(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    use anyhow::Context;

    log::info!("Collecting patch files of directory {:?}...", dir);

    let mut entries = Vec::new();
    let mut dir_reader = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("can't read folder {:?}", dir))?;
    while let Some(entry) = dir_reader.next_entry().await.with_context(|| format!("can't read folder {:?}", dir))? {
        if !entry.file_type()
            .await
            .map(|t| t.is_file())
            .unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if !path.file_name()
            .and_then(|n| n.to_str())
            .map(is_patch_filename)
            .unwrap_or(false) {
            continue;
        }
        entries.push(path);
    }

    log::info!("Found {} patch files.", entries.len());
    Ok(entries)
}

async fn add_files_from_dir(dir: &Path, groups: &mut HashMap<String, Vec<PatchFileTriplet>>) -> anyhow::Result<()> {
    let index_regex = INDEX_REGEX.get_or_init(|| Regex::new(r"\.patch_(\d+)").unwrap());

    let entries = get_patch_files_from_dir(dir).await?;

    let names: HashSet<String> = entries
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(|s| s[..16].to_string()))
        .collect();

    for name in names {
        let mut indices: Vec<u32> = entries
            .iter()
            .filter_map(|p| {
                let fname = p.file_name()?.to_str()?;
                if !fname.starts_with(&*name) {
                    return None;
                }
                let caps = index_regex.captures(fname)?;
                caps[1].parse().ok()
            })
            .collect::<HashSet<u32>>()
            .into_iter()
            .collect();
        // A mod's own patch files keep their relative order: its patch_0
        // is deployed before (below) its patch_1. A HashSet alone would
        // shuffle them.
        indices.sort_unstable();

        for index in indices {
            let patch = entries.iter().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == format!("{}.patch_{}", name, index))
                    .unwrap_or(false)
            }).cloned();

            let gpu_resources = entries.iter().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == format!("{}.patch_{}.gpu_resources", name, index))
                    .unwrap_or(false)
            }).cloned();

            let stream = entries.iter().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == format!("{}.patch_{}.stream", name, index))
                    .unwrap_or(false)
            }).cloned();

            groups
                .entry(name.clone())
                .or_default()
                .push(PatchFileTriplet { patch, gpu_resources, stream });
        }
    }

    Ok(())
}

async fn copy_patch_file(src: &Path, dest: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    tokio::fs::copy(src, dest)
        .await
        .with_context(|| format!("failed to copy {:?} to {:?}", src, dest))?;
    Ok(())
}

async fn create_empty_patch_file(dest: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("failed to create {:?}", dest))?;
    Ok(())
}

/// The record of which patch files the last deploy wrote, kept next to
/// them in the game's `data` folder (so it always describes that folder,
/// whichever DDMM data folder or game path is in use). Purge removes it.
///
/// A deploy writes it twice: before copying anything, as a pending record
/// of every file it is about to write (see [`PendingFile`]), and after
/// copying, as the final record of what landed (see [`RecordedFile`]). So
/// a deploy that is interrupted at any point (a crash, the app killed)
/// still leaves every file it wrote on record.
const DEPLOY_RECORD: &str = ".ddmm-deployed.json";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DeployRecord {
    /// The files a finished deploy wrote (empty while it is pending).
    files: Vec<RecordedFile>,
    /// The files a deploy that hasn't finished is writing. A record with
    /// these was left by a deploy that stopped before it could write its
    /// final record. Versions without pending records ignore the field and
    /// see no files, so they keep a skip-listed slot 0 (the safe side).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<PendingFile>,
}

/// One file a deploy wrote, with its length and modification time
/// (nanoseconds since the Unix epoch) right after it was written. If either
/// has changed since, something else (a Steam "verify files", the user)
/// replaced it, and it is no longer DDMM's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RecordedFile {
    name: String,
    len: u64,
    modified: u64,
}

/// One file a deploy is about to write, recorded before it copies
/// anything.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PendingFile {
    name: String,
    /// The length it will have once written (its source's, or 0 for an
    /// empty placeholder). A copy that was cut short is shorter, never
    /// longer, so a longer file there isn't DDMM's.
    len: u64,
    /// The mod file copied there, or `None` for an empty placeholder. A
    /// file there is DDMM's only while its bytes are (a cut-short copy of)
    /// this file's.
    #[serde(default)]
    source: Option<PathBuf>,
    /// Whether a file of that name was already there before the deploy
    /// copied anything (deploy purges first, so normally not). Only a file
    /// that wasn't there can be DDMM's: one that was is someone else's.
    existed: bool,
}

/// A file's length and modification time, as recorded in [`RecordedFile`].
async fn file_stamp(file: &Path) -> Option<(u64, u64)> {
    let meta = tokio::fs::symlink_metadata(file).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((meta.len(), u64::try_from(modified.as_nanos()).ok()?))
}

/// What the last deploy recorded writing, by file name.
#[derive(Default)]
struct Deployed {
    files: HashMap<String, RecordedFile>,
    pending: HashMap<String, PendingFile>,
}

impl Deployed {
    /// Whether `file` (in the game's `data` folder) is, as far as the
    /// record shows, a file DDMM wrote and nothing has replaced since: the
    /// final record lists it with its current length and modification
    /// time, or a pending record lists it as written by a deploy that
    /// stopped, where nothing of that name was before and its bytes are
    /// still DDMM's copy (see [`is_copy_of`]).
    async fn wrote(&self, file: &Path) -> bool {
        let Some(name) = file.file_name().and_then(|n| n.to_str()) else { return false };
        let Some(stamp) = file_stamp(file).await else { return false };
        if let Some(r) = self.files.get(name) {
            return stamp == (r.len, r.modified);
        }
        let Some(p) = self.pending.get(name) else { return false };
        if p.existed || stamp.0 > p.len {
            return false;
        }
        let (file, source) = (file.to_path_buf(), p.source.clone());
        tokio::task::spawn_blocking(move || is_copy_of(&file, source.as_deref()))
            .await
            .unwrap_or(false)
    }
}

/// Whether `file` holds what copying `source` there (or, for `None`,
/// creating an empty placeholder) leaves, possibly cut short: the start of
/// `source`'s bytes, optionally followed by zeros (space a copy had
/// reserved but not yet filled). A file that differs anywhere else isn't
/// DDMM's. Nor is anything when `source` is gone or can't be read: then
/// it can't be shown to be DDMM's, so it is kept.
///
/// A game file whose bytes happen to be exactly the mod's is
/// indistinguishable from DDMM's copy; deleting it removes nothing but
/// those same bytes.
fn is_copy_of(file: &Path, source: Option<&Path>) -> bool {
    let Ok(mut file) = std::fs::File::open(file) else { return false };
    let mut source = match source {
        Some(s) => match std::fs::File::open(s) {
            Ok(s) => Some(s),
            Err(e) => {
                log::info!("Can't read {:?} ({e}), the mod file a stopped deploy was copying; keeping its copy.", s);
                return false;
            }
        },
        None => None,
    };
    const CHUNK: usize = 1 << 16;
    let (mut a, mut b) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut in_zeros = false;
    loop {
        let n = match read_full(&mut file, &mut a) {
            Ok(0) => return true,
            Ok(n) => n,
            Err(_) => return false,
        };
        let m = if in_zeros {
            0
        } else {
            match source.as_mut().map(|s| read_full(s, &mut b[..n])) {
                Some(Ok(m)) => m,
                Some(Err(_)) => return false,
                None => 0,
            }
        };
        // `a[..n]` must start with `b[..m]`; once they part, only zeros.
        let same = a[..m].iter().zip(&b[..m]).take_while(|(x, y)| x == y).count();
        if same < m || m < n {
            in_zeros = true;
        }
        if a[same..n].iter().any(|&x| x != 0) {
            return false;
        }
    }

    /// Read until `buf` is full or the file ends.
    fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::Read;
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        Ok(n)
    }
}

/// The files the last deploy recorded writing, or `None` when there is no
/// usable record (none yet -- including a game folder last deployed by a
/// version without records -- or an unreadable or damaged one).
async fn read_deploy_record(data_dir: &Path) -> Option<Deployed> {
    let file = data_dir.join(DEPLOY_RECORD);
    let data = match tokio::fs::read(&file).await {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("Couldn't read the deploy record {:?}: {}", file, e);
            return None;
        }
    };
    match serde_json::from_slice::<DeployRecord>(&data) {
        Ok(record) => Some(Deployed {
            files: record
                .files
                .into_iter()
                .filter(|f| is_patch_filename(&f.name))
                .map(|f| (f.name.clone(), f))
                .collect(),
            pending: record
                .pending
                .into_iter()
                .filter(|f| is_patch_filename(&f.name))
                .map(|f| (f.name.clone(), f))
                .collect(),
        }),
        Err(e) => {
            log::warn!("Ignoring the damaged deploy record {:?}: {}", file, e);
            None
        }
    }
}

/// Replace the deploy record in `data_dir` with `record`, durably: once
/// this returns, a crash can't leave the old record (or none) in its place.
async fn write_deploy_record(data_dir: &Path, record: &DeployRecord) -> anyhow::Result<()> {
    let file = data_dir.join(DEPLOY_RECORD);
    let data = serde_json::to_vec_pretty(record)?;
    tokio::task::spawn_blocking(move || crate::fs_util::replace_file_durably_blocking(&file, &data)).await?
}

/// The hint every "couldn't write the deploy record" error ends with.
fn record_write_hint() -> String {
    format!("check that the game's data folder isn't read-only and that nothing is in the way of {DEPLOY_RECORD} there, then deploy again")
}

/// Check, before deploy purges anything, that the deploy record can be
/// written: replace it with itself (its bytes unchanged), or, with none
/// there, with an empty record (which purge reads like no record). A
/// deploy that fails this stops with the previous deploy still in place.
async fn probe_deploy_record(data_dir: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let file = data_dir.join(DEPLOY_RECORD);
    let data = match tokio::fs::read(&file).await {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => br#"{"Files":[]}"#.to_vec(),
        // Unreadable: purge treats it as no record, so an empty one
        // changes nothing (and the write below fails if it is in the way).
        Err(_) => br#"{"Files":[]}"#.to_vec(),
    };
    let written = if crash_point(DeployCrashPoint::ProbeFails) {
        Err(anyhow::anyhow!("simulated failure"))
    } else {
        tokio::task::spawn_blocking(move || crate::fs_util::replace_file_durably_blocking(&file, &data))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r)
    };
    written.with_context(|| {
            format!(
                "couldn't write DDMM's deploy record in {:?}, so nothing was changed (the mods deployed before are still in place); {}",
                data_dir,
                record_write_hint()
            )
        })
}

/// Record, before copying anything, every file this deploy is about to
/// write (`plan`), and whether something of that name is already there. A
/// deploy that can't write this must not copy anything: the files would
/// be on no record.
async fn record_pending_files(data_dir: &Path, plan: &[PlannedCopy]) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut pending = Vec::with_capacity(plan.len());
    for PlannedCopy { name, src, len } in plan {
        let existed = match tokio::fs::symlink_metadata(data_dir.join(name)).await {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                return Err(anyhow::Error::from(e).context(format!("couldn't check {:?} in the game's data folder", name)))
            }
        };
        if existed {
            log::warn!("{:?} is in the game's data folder although the purge just ran; it is overwritten, but not taken for DDMM's.", name);
        }
        pending.push(PendingFile { name: name.clone(), len: *len, source: src.clone(), existed });
    }
    let written = if crash_point(DeployCrashPoint::PendingRecordFails) {
        Err(anyhow::anyhow!("simulated failure"))
    } else {
        write_deploy_record(data_dir, &DeployRecord { files: Vec::new(), pending }).await
    };
    written.with_context(|| {
        format!(
            "couldn't write DDMM's deploy record in {:?}, so nothing was deployed (the mods deployed before were already removed); {}",
            data_dir,
            record_write_hint()
        )
    })
}

/// Record which of `names` (in `data_dir`) are now there, as written by
/// this deploy, replacing its pending record. Tried twice; if it still
/// fails, the deploy reports it (the files are in place, and the pending
/// record still covers them).
async fn record_deployed_files(data_dir: &Path, names: &[String]) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut files = Vec::with_capacity(names.len());
    for name in names {
        if let Some((len, modified)) = file_stamp(&data_dir.join(name)).await {
            files.push(RecordedFile { name: name.clone(), len, modified });
        }
    }
    let record = DeployRecord { files, pending: Vec::new() };
    let mut result = Err(anyhow::anyhow!("not written"));
    for attempt in 0..2 {
        result = if crash_point(DeployCrashPoint::FinalRecordFails) {
            Err(anyhow::anyhow!("simulated failure"))
        } else {
            write_deploy_record(data_dir, &record).await
        };
        match &result {
            Ok(()) => break,
            Err(e) => log::warn!("Couldn't write the final deploy record in {:?} (attempt {}): {:#}", data_dir, attempt + 1, e),
        }
    }
    result.with_context(|| {
        format!(
            "the mods were deployed, but DDMM couldn't finish its deploy record in {:?} (its record from before copying still covers them, so Purge will still remove them); {}",
            data_dir,
            record_write_hint()
        )
    })
}

/// Whether `file` (a patch file in the game's `data` folder) is slot 0 of a
/// patch name in the Skip List: the base game's or a DLC's own file, which
/// deploy numbers around (it starts that name at `.patch_1`) and purge
/// leaves alone -- unless the deploy record says DDMM wrote it (deployed
/// before the name was added to the Skip List).
fn is_skip_listed_slot_zero(file: &Path, settings: &Settings) -> bool {
    let Some(name) = file.file_name().and_then(|n| n.to_str()) else { return false };
    let Some((prefix, rest)) = name.split_at_checked(16) else { return false };
    matches!(rest, ".patch_0" | ".patch_0.gpu_resources" | ".patch_0.stream") && settings.has_skip_entry(prefix)
}

/// Temporary files a record write interrupted by a crash left behind
/// (`.ddmm-deployed.json.<id>.tmp`, see [`crate::fs_util::replace_file`]).
async fn leftover_record_temp_files(data_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(mut dir) = tokio::fs::read_dir(data_dir).await else { return found };
    let prefix = format!("{DEPLOY_RECORD}.");
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name.strip_prefix(&prefix).and_then(|n| n.strip_suffix(".tmp")) else { continue };
        if uuid::Uuid::try_parse(id).is_ok() && entry.file_type().await.is_ok_and(|t| t.is_file()) {
            found.push(entry.path());
        }
    }
    found
}

async fn do_purge(data_dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    use anyhow::Context;
    log::info!("Purging...");

    let deployed = read_deploy_record(data_dir).await;
    if deployed.as_ref().is_some_and(|d| !d.pending.is_empty()) {
        log::info!("The last deploy didn't finish; removing the files it wrote.");
    }
    let mut patch_files = Vec::new();
    for f in get_patch_files_from_dir(data_dir).await? {
        if is_skip_listed_slot_zero(&f, settings) {
            // Only delete it if DDMM wrote it and nothing has replaced it
            // since; otherwise it is (again) the game's own file.
            let ours = match &deployed {
                Some(d) => d.wrote(&f).await,
                None => false,
            };
            if !ours {
                log::info!("Keeping {:?}: its patch name is in the Skip List and it isn't a file DDMM deployed.", f);
                continue;
            }
        }
        patch_files.push(f);
    }

    log::info!("Deleting files...");
    futures::future::try_join_all(patch_files.iter().map(|f| async move {
        tokio::fs::remove_file(f)
            .await
            .with_context(|| format!("failed to delete {:?}", f))
    }))
    .await?;

    if crash_point(DeployCrashPoint::PurgeBeforeRecordRemoved) {
        anyhow::bail!("simulated crash");
    }

    // Everything DDMM deployed is gone, so the record is too.
    let record = data_dir.join(DEPLOY_RECORD);
    // A record that can't be removed stays until the next deploy replaces
    // it. Meanwhile it only claims what is still DDMM's: a final entry
    // only a file with the exact length and modification time it
    // recorded, a pending entry only a file whose bytes are still a copy
    // of its mod file (see `Deployed::wrote`).
    match tokio::fs::remove_file(&record).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("Couldn't delete the deploy record {:?}: {}", record, e),
    }
    for temp in leftover_record_temp_files(data_dir).await {
        if let Err(e) = tokio::fs::remove_file(&temp).await {
            log::warn!("Couldn't delete the leftover temporary file {:?}: {}", temp, e);
        }
    }

    log::info!("Purge complete.");
    Ok(())
}

/// Where a test stops a deploy or purge, as if DDMM crashed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeployCrashPoint {
    /// Purge deleted the files; the record isn't removed yet.
    PurgeBeforeRecordRemoved,
    /// Deploy purged; the pending record isn't written yet.
    BeforePendingRecord,
    /// The pending record is written; nothing copied yet.
    BeforeCopy,
    /// The first file is copied, the rest not.
    AfterFirstFile,
    /// Everything is copied; the final record isn't written yet.
    BeforeFinalRecord,
    /// Not a crash: the record write deploy tries before purging fails.
    ProbeFails,
    /// Not a crash: writing the pending record fails.
    PendingRecordFails,
    /// Not a crash: writing the final record fails (every attempt).
    FinalRecordFails,
}

#[cfg(test)]
thread_local! {
    static DEPLOY_CRASH_AT: std::cell::Cell<Option<DeployCrashPoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn crash_point(at: DeployCrashPoint) -> bool {
    DEPLOY_CRASH_AT.with(|c| c.get()) == Some(at)
}

#[cfg(not(test))]
#[inline(always)]
fn crash_point(_: DeployCrashPoint) -> bool {
    false
}

/// Pair each config with the installed mod it refers to (by GUID),
/// dropping any config whose GUID doesn't match an installed mod. A
/// user-edited (or stale) `profiles.json` can easily reference a mod that
/// no longer exists; that's not an error, the entry is just skipped.
fn pair_mods_with_configs<'a>(mods: &'a [Mod], configs: &'a [Config]) -> Vec<(&'a Mod, &'a Config)> {
    let by_guid = mods.iter()
        .map(|m| (m.guid(), m))
        .collect::<HashMap<_, _>>();

    configs.iter()
        .filter_map(|c| {
            let r#mod = by_guid.get(c.uuid()).copied();
            if r#mod.is_none() {
                log::warn!("skipping config for unknown mod GUID {{{}}}", c.uuid());
            }
            r#mod.map(|m| (m, c))
        })
        .collect()
}

/// Collect the enabled option (and, for options with sub-options, the
/// selected sub-option's) include directories for one mod into `groups`.
/// Skips a disabled mod. Never panics: a version mismatch between the
/// manifest and its config (which should never happen from the app's own
/// UI, but can from a hand-edited `profiles.json`), an out-of-range
/// selected index, or an unknown GUID are all just logged and skipped.
///
/// Factored out of `deploy` so it's testable without going through
/// `State`/`AppState` at all -- just a `Mod` and a `Config`.
async fn collect_files_for_mod(
    r#mod: &Mod,
    config: &Config,
    groups: &mut HashMap<String, Vec<PatchFileTriplet>>,
) -> anyhow::Result<()> {
    if !config.enabled() {
        return Ok(());
    }

    match (&r#mod.manifest, config) {
        (Manifest::Legacy(manifest), Config::Legacy { selected, .. }) => {
            let base = &r#mod.directory;

            if let Some(options) = manifest.options.as_ref() {
                if let Some(opt) = options.get(*selected) {
                    let dir = resolve_mod_path(base, Path::new(opt)).await;
                    add_files_from_dir(&dir, groups).await?;
                } else {
                    log::warn!(
                        "mod {{{}}}: selected option index {} out of range",
                        r#mod.guid(), selected
                    );
                }
            } else {
                add_files_from_dir(base, groups).await?;
            }
        }
        (Manifest::V1(manifest), Config::V1 { selected, toggled, .. }) => {
            let base = &r#mod.directory;

            if let Some(options) = manifest.options.as_ref() {
                for (i, opt) in options.iter().enumerate() {
                    if !toggled.get(i).copied().unwrap_or(false) {
                        continue;
                    }

                    if let Some(includes) = opt.include.as_ref() {
                        for inc in includes {
                            let dir = resolve_mod_path(base, inc).await;
                            add_files_from_dir(&dir, groups).await?;
                        }
                    }

                    if let Some(sub_options) = opt.sub_options.as_ref() {
                        if let Some(idx) = selected.get(i).cloned() {
                            if let Some(sub) = sub_options.get(idx) {
                                for inc in &sub.include {
                                    let dir = resolve_mod_path(base, inc).await;
                                    add_files_from_dir(&dir, groups).await?;
                                }
                            } else {
                                log::warn!(
                                    "mod {{{}}}: selected sub-option index {} out of range for option {}",
                                    r#mod.guid(), idx, i
                                );
                            }
                        }
                    }
                }
            } else {
                add_files_from_dir(base, groups).await?;
            }
        }
        // V2's Option/SubOption shapes are structurally the same as V1's
        // (plus a Guid/CategoryRef that deploy doesn't need), and
        // Config::V2's Toggled/Selected are index-keyed exactly like
        // Config::V1's -- see ModConfigPopup / makeConfigForMod on the
        // frontend, which is what this is derived from (the upstream v2
        // draft doesn't specify config shape at all). So V2 deploys
        // identically to V1.
        (Manifest::V2(manifest), Config::V2 { selected, toggled, .. }) => {
            let base = &r#mod.directory;

            if let Some(options) = manifest.options.as_ref() {
                for (i, opt) in options.iter().enumerate() {
                    if !toggled.get(i).copied().unwrap_or(false) {
                        continue;
                    }

                    if let Some(includes) = opt.include.as_ref() {
                        for inc in includes {
                            let dir = resolve_mod_path(base, inc).await;
                            add_files_from_dir(&dir, groups).await?;
                        }
                    }

                    if let Some(sub_options) = opt.sub_options.as_ref() {
                        if let Some(idx) = selected.get(i).cloned() {
                            if let Some(sub) = sub_options.get(idx) {
                                for inc in &sub.include {
                                    let dir = resolve_mod_path(base, inc).await;
                                    add_files_from_dir(&dir, groups).await?;
                                }
                            } else {
                                log::warn!(
                                    "mod {{{}}}: selected sub-option index {} out of range for option {}",
                                    r#mod.guid(), idx, i
                                );
                            }
                        }
                    }
                }
            } else {
                add_files_from_dir(base, groups).await?;
            }
        }
        _ => {
            log::warn!(
                "mod {{{}}}: manifest version and config version don't match, skipping",
                r#mod.guid()
            );
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn deploy(state: State<'_, AppState>, configs: Vec<Config>) -> TAResult<()> {
    let _data_op = state.data_op().into_ta_result()?;
    let mods = state.inner().mods.lock().await;
    if mods.is_none() {
        return anyhow::anyhow!("mods not read").into_ta_result();
    }
    let mods = mods.as_ref().unwrap();

    let settings = do_load_settings(&state.base_path).await?;
    let game_root = match settings.validate().await {
        Ok(root) => root,
        Err(e) => return anyhow::anyhow!("invalid settings: {}", e).into_ta_result(),
    };

    let mods = pair_mods_with_configs(mods, &configs);

    let data_dir = game_root.join("data");
    crate::commands::settings::check_game_data_dir(&state.base_path, &data_dir).into_ta_result()?;

    do_deploy(&data_dir, &settings, mods).await.into_ta_result()
}

/// Deploy `mods` into the game's `data_dir`: collect every enabled mod's
/// patch files first, then check the record can be written, then purge,
/// then record what is about to be written, then copy, then record what
/// was written. Collecting and checking first means a mod that can't be
/// deployed (an option folder missing from it, an unreadable folder) or a
/// game folder that won't take the record stops the deploy before
/// anything in the game folder is touched, so the previous deploy stays in
/// place instead of the game being left with no mods at all. Recording
/// before copying means a deploy stopped at any point (a crash, the app
/// killed) leaves every file it wrote on record for the next purge; a
/// deploy that can't write that record copies nothing.
async fn do_deploy(data_dir: &Path, settings: &Settings, mods: Vec<(&Mod, &Config)>) -> anyhow::Result<()> {
    log::info!("Grouping files...");
    let mut groups: HashMap<String, Vec<PatchFileTriplet>> = HashMap::new();
    for (r#mod, config) in mods {
        collect_files_for_mod(r#mod, config, &mut groups).await?;
    }

    log::info!("Collected files into {} groups.", groups.len());
    if log::log_enabled!(log::Level::Debug) {
        for (k, v) in &groups {
            log::debug!(" - k: \"{}\"; v: [{}]", k, v.len());
        }
    }

    let plan = plan_copies(settings, &groups).await?;

    // Before touching the game folder, make sure the record can be
    // written, so a folder that won't take it leaves the previous deploy
    // in place rather than purged.
    if !plan.is_empty() {
        probe_deploy_record(data_dir).await?;
    }

    do_purge(data_dir, settings).await?;

    log::info!("Deploying...");

    if plan.is_empty() {
        log::info!("Nothing to deploy.");
        return Ok(());
    }

    if crash_point(DeployCrashPoint::BeforePendingRecord) {
        anyhow::bail!("simulated crash");
    }
    record_pending_files(data_dir, &plan).await?;
    if crash_point(DeployCrashPoint::BeforeCopy) {
        anyhow::bail!("simulated crash");
    }

    log::info!("Copying files...");
    let (touched, copied) = copy_files(data_dir, &plan).await;
    if crash_point(DeployCrashPoint::AfterFirstFile) || crash_point(DeployCrashPoint::BeforeFinalRecord) {
        return copied.and(Err(anyhow::anyhow!("simulated crash")));
    }
    // Recorded even when copying stopped partway, so the files that did
    // land (including one cut short by the failed copy) are known to be
    // DDMM's.
    let mut written: Vec<String> = plan.into_iter().take(touched).map(|c| c.name).collect();
    written.sort();
    let recorded = record_deployed_files(data_dir, &written).await;
    copied?;
    recorded?;

    log::info!("Deployment complete.");
    Ok(())
}

/// One file deploy writes into the game's `data` folder.
struct PlannedCopy {
    /// Its name there.
    name: String,
    /// The mod file copied there, or `None` for an empty placeholder.
    src: Option<PathBuf>,
    /// The length it will have: its source's, or 0.
    len: u64,
}

/// Every file deploy writes for `groups`, in the order it writes them.
///
/// Load order: `configs` is the profile's mod list, top to bottom, and
/// each group's triplets were collected in that order. The first gets
/// `<name>.patch_0` (after an optional skip-list offset), the next
/// `.patch_1`, and so on. Helldivers 2 applies higher patch numbers over
/// lower ones, so the mod LOWEST in the list wins a conflict. A triplet
/// missing its `.gpu_resources` or `.stream` gets an empty placeholder, so
/// the numbering stays consistent for later mods touching the same name.
async fn plan_copies(settings: &Settings, groups: &HashMap<String, Vec<PatchFileTriplet>>) -> anyhow::Result<Vec<PlannedCopy>> {
    use anyhow::Context;
    let mut plan = Vec::new();
    // By name, so the copy order (and where an interrupted deploy stops)
    // doesn't depend on the map's.
    let mut names: Vec<&String> = groups.keys().collect();
    names.sort();
    for name in names {
        let triplets = &groups[name];
        let offset = if settings.has_skip_entry(name) { 1 } else { 0 };
        for (i, triplet) in triplets.iter().enumerate() {
            let index = i + offset;
            for (suffix, src) in [("", &triplet.patch), (".gpu_resources", &triplet.gpu_resources), (".stream", &triplet.stream)] {
                let len = match src {
                    Some(src) => tokio::fs::metadata(src)
                        .await
                        .with_context(|| format!("can't read {:?}", src))?
                        .len(),
                    None => 0,
                };
                plan.push(PlannedCopy { name: format!("{}.patch_{}{}", name, index, suffix), src: src.clone(), len });
            }
        }
    }
    Ok(plan)
}

/// Copy `plan` into `data_dir`, in order. Returns how many of its files
/// were copied or started (a failed copy may have left its file cut
/// short), and whether all were.
async fn copy_files(data_dir: &Path, plan: &[PlannedCopy]) -> (usize, anyhow::Result<()>) {
    for (i, copy) in plan.iter().enumerate() {
        if i == 1 && crash_point(DeployCrashPoint::AfterFirstFile) {
            return (i, Ok(()));
        }
        let dest = data_dir.join(&copy.name);
        let done = match &copy.src {
            Some(src) => copy_patch_file(src, &dest).await,
            None => create_empty_patch_file(&dest).await,
        };
        if let Err(e) = done {
            return (i + 1, Err(e));
        }
    }
    (plan.len(), Ok(()))
}

#[tauri::command]
pub async fn purge(state: State<'_, AppState>) -> TAResult<()> {
    let _data_op = state.data_op().into_ta_result()?;
    let settings = do_load_settings(&state.base_path).await.into_ta_result()?;
    let game_root = match settings.validate().await {
        Ok(root) => root,
        Err(e) => return anyhow::anyhow!("invalid settings: {}", e).into_ta_result(),
    };

    let data_dir = game_root.join("data");
    crate::commands::settings::check_game_data_dir(&state.base_path, &data_dir).into_ta_result()?;
    do_purge(&data_dir, &settings).await.into_ta_result()
}

/// Last-resort way to close the app from the frontend.
///
/// Normally the window closes itself via the `window|destroy` IPC call
/// that `@tauri-apps/api`'s `onCloseRequested` wrapper makes once our close
/// handler resolves. This command exists for the case where that call
/// fails for some other reason (the permission is granted as of this
/// release, but a future regression there, a webview IPC hiccup, etc.
/// shouldn't be able to strand a user with an unclosable window again) --
/// the frontend falls back to this after logging the `destroy` error.
///
/// This intentionally does not go through `AppState` or try to save
/// anything: by the time the frontend reaches for this fallback it has
/// already given the user a chance to save/confirm, and the whole point is
/// to guarantee the process actually exits.
#[tauri::command]
pub fn force_exit(app: tauri::AppHandle) {
    log::warn!("force_exit invoked; exiting immediately.");
    app.exit(0);
}

/// Called by the frontend's close handler as the very first thing it does,
/// proving to the Rust-side close watchdog (see `lib.rs`) that the
/// frontend is alive and actually handling the close request.
#[tauri::command]
pub fn ack_close_requested(state: State<'_, AppState>) {
    state.close_ack.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Open the folder with DDMM's log files in the file manager (the Mods
/// page offers this when it couldn't load, so the log is one click away for
/// a bug report). Opened from here rather than with the opener plugin from
/// the page, so it works wherever the data folder is.
#[tauri::command]
pub fn open_log_folder(app: tauri::AppHandle, state: State<'_, AppState>) -> TAResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = crate::data_dir::log_dir(&state.data_dir);
    log::info!("Opening the log folder {:?}.", dir);
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| anyhow::anyhow!("couldn't open the log folder {}: {e}", dir.display()))
        .into_ta_result()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::manifest::legacy;
    use uuid::Uuid;

    const V2_FIXTURE: &str = include_str!("../../tests/fixtures/v2_manifest_fixture.json");

    fn v2_fixture_manifest() -> Manifest {
        serde_json::from_str(V2_FIXTURE).expect("fixture should parse as a manifest")
    }

    fn patch_file_name(index: u32) -> String {
        format!("0123456789abcdef.patch_{}", index)
    }

    async fn write_patch_file(dir: &std::path::Path, index: u32) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        tokio::fs::write(dir.join(patch_file_name(index)), b"data").await.unwrap();
    }

    /// Lays out the fixture's expected directories (Base, Variants/Red,
    /// Variants/Blue), each with one distinguishable patch file, under a
    /// fresh temp dir, and returns a Mod pointing at it with the fixture
    /// manifest.
    async fn v2_fixture_mod() -> (tempfile::TempDir, Mod) {
        let dir = tempfile::tempdir().unwrap();
        write_patch_file(&dir.path().join("Base"), 0).await;
        write_patch_file(&dir.path().join("Variants/Red"), 1).await;
        write_patch_file(&dir.path().join("Variants/Blue"), 2).await;

        let r#mod = Mod {
            manifest: v2_fixture_manifest(),
            directory: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        (dir, r#mod)
    }

    fn v2_guid() -> Uuid {
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
    }

    #[tokio::test]
    async fn windows_style_include_paths_resolve_on_case_sensitive_filesystems() {
        // A Windows-authored manifest: backslash separators and casing
        // that doesn't match the folders on disk.
        let dir = tempfile::tempdir().unwrap();
        write_patch_file(&dir.path().join("Variants/Red"), 0).await;
        let r#mod = Mod {
            manifest: Manifest::Legacy(legacy::Manifest {
                guid: Uuid::nil(),
                name: "win".into(),
                description: String::new(),
                icon_path: None,
                options: Some(vec!["variants\\red".into()]),
            }),
            directory: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        let config = Config::Legacy { guid: Uuid::nil(), enabled: true, selected: 0 };

        let mut groups = HashMap::new();
        collect_files_for_mod(&r#mod, &config, &mut groups).await.unwrap();
        assert!(groups.contains_key("0123456789abcdef"));
    }

    #[tokio::test]
    async fn missing_include_folder_error_names_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let r#mod = Mod {
            manifest: Manifest::Legacy(legacy::Manifest {
                guid: Uuid::nil(),
                name: "m".into(),
                description: String::new(),
                icon_path: None,
                options: Some(vec!["NotThere".into()]),
            }),
            directory: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        let config = Config::Legacy { guid: Uuid::nil(), enabled: true, selected: 0 };

        let err = collect_files_for_mod(&r#mod, &config, &mut HashMap::new()).await.unwrap_err();
        assert!(format!("{:#}", err).contains("NotThere"), "{err:#}");
    }

    #[tokio::test]
    async fn v2_deploy_collects_enabled_option_files() {
        let (_dir, r#mod) = v2_fixture_mod().await;
        let config = Config::V2 {
            guid: v2_guid(),
            enabled: true,
            toggled: vec![true, false], // Base Option on, Color Variant off
            selected: vec![0, 0],
        };

        let mut groups = HashMap::new();
        collect_files_for_mod(&r#mod, &config, &mut groups).await.unwrap();

        assert_eq!(groups.len(), 1, "only the Base option's patch file should be grouped");
        assert!(groups.contains_key("0123456789abcdef"));
    }

    #[tokio::test]
    async fn v2_deploy_collects_selected_sub_option_files() {
        let (_dir, r#mod) = v2_fixture_mod().await;
        // Both options on; Color Variant's selected sub-option is index 1 (Blue).
        let config = Config::V2 {
            guid: v2_guid(),
            enabled: true,
            toggled: vec![true, true],
            selected: vec![0, 1],
        };

        let mut groups = HashMap::new();
        collect_files_for_mod(&r#mod, &config, &mut groups).await.unwrap();

        // Base + Blue both write into the same 16-hex-char group name here
        // (fixture reuses the name for simplicity), so assert via patch count instead.
        let total_patches: usize = groups.values().map(|v| v.len()).sum();
        assert_eq!(total_patches, 2, "Base + Blue, but not Red");
    }

    /// #55: the sub-option picked in the popup (stored as `Selected` in
    /// the profile) decides which folder is deployed, for every option.
    #[tokio::test]
    async fn v1_deploy_uses_the_selected_sub_option_of_each_option() {
        let dir = tempfile::tempdir().unwrap();
        for (i, folder) in ["Core", "Core/None", "Core/Half", "Primaries/None", "Primaries/Half", "Extra"]
            .iter()
            .enumerate()
        {
            write_patch_file(&dir.path().join(folder), i as u32).await;
        }
        let manifest = Manifest::parse(
            include_bytes!("../../tests/fixtures/manifests/sub_options_v1.json"),
            "sub_options_v1.json",
        )
        .unwrap();
        let r#mod = Mod { manifest, directory: dir.path().to_path_buf(), sources: Vec::new() };
        let guid = r#mod.guid();

        // A profile saved by the popup: Half sway for core, no sway for primaries.
        let config: Config = serde_json::from_value(serde_json::json!({
            "For": "V1", "Guid": guid, "Enabled": true,
            "Toggled": [true, true, true], "Selected": [1, 0, 0],
        }))
        .unwrap();
        assert_eq!(serde_json::to_value(&config).unwrap()["Selected"], serde_json::json!([1, 0, 0]));

        let mut groups = HashMap::new();
        collect_files_for_mod(&r#mod, &config, &mut groups).await.unwrap();
        let mut deployed: Vec<PathBuf> = groups
            .values()
            .flatten()
            .map(|t| t.patch.clone().unwrap().strip_prefix(dir.path()).unwrap().parent().unwrap().to_path_buf())
            .collect();
        deployed.sort();
        let expected: Vec<PathBuf> = ["Core", "Core/Half", "Extra", "Primaries/None"].iter().map(PathBuf::from).collect();
        assert_eq!(deployed, expected);

        // The default entry (first sub-option everywhere) deploys the first choices.
        let config = Config::V1 { guid, enabled: true, toggled: vec![true, true, false], selected: vec![0, 0, 0] };
        let mut groups = HashMap::new();
        collect_files_for_mod(&r#mod, &config, &mut groups).await.unwrap();
        let mut deployed: Vec<PathBuf> = groups
            .values()
            .flatten()
            .map(|t| t.patch.clone().unwrap().strip_prefix(dir.path()).unwrap().parent().unwrap().to_path_buf())
            .collect();
        deployed.sort();
        let expected: Vec<PathBuf> = ["Core", "Core/None", "Primaries/None"].iter().map(PathBuf::from).collect();
        assert_eq!(deployed, expected);
    }

    #[tokio::test]
    async fn v2_deploy_skips_disabled_mod() {
        let (_dir, r#mod) = v2_fixture_mod().await;
        let config = Config::V2 {
            guid: v2_guid(),
            enabled: false,
            toggled: vec![true, true],
            selected: vec![0, 0],
        };

        let mut groups = HashMap::new();
        collect_files_for_mod(&r#mod, &config, &mut groups).await.unwrap();

        assert!(groups.is_empty());
    }

    #[test]
    fn unknown_guid_in_config_is_skipped_not_panicked() {
        let (guid, manifest) = (v2_guid(), v2_fixture_manifest());
        let r#mod = Mod {
            manifest,
            directory: PathBuf::from("/nonexistent"),
            sources: Vec::new(),
        };
        let mods = vec![r#mod];

        let unrelated_guid = Uuid::parse_str("99999999-9999-9999-9999-999999999999").unwrap();
        assert_ne!(unrelated_guid, guid);

        let configs = vec![Config::V2 {
            guid: unrelated_guid,
            enabled: true,
            toggled: vec![true],
            selected: vec![0],
        }];

        let paired = pair_mods_with_configs(&mods, &configs);
        assert!(paired.is_empty(), "a config for an unknown GUID must be dropped, not matched");
    }

    #[tokio::test]
    async fn mismatched_manifest_and_config_versions_do_not_panic() {
        let (_dir, r#mod) = v2_fixture_mod().await; // Manifest::V2
        let config = Config::V1 {
            guid: v2_guid(),
            enabled: true,
            toggled: vec![true],
            selected: vec![0],
        };

        let mut groups = HashMap::new();
        // Must not panic (no todo!/unreachable!) and must not collect anything.
        let result = collect_files_for_mod(&r#mod, &config, &mut groups).await;

        assert!(result.is_ok());
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn legacy_manifest_config_mismatch_also_does_not_panic() {
        let guid = Uuid::parse_str("77777777-7777-7777-7777-777777777777").unwrap();
        let r#mod = Mod {
            manifest: Manifest::Legacy(legacy::Manifest {
                guid,
                name: "Legacy Mod".to_string(),
                description: String::new(),
                icon_path: None,
                options: None,
            }),
            directory: PathBuf::from("/nonexistent"),
            sources: Vec::new(),
        };
        let config = Config::V2 {
            guid,
            enabled: true,
            toggled: vec![],
            selected: vec![],
        };

        let mut groups = HashMap::new();
        let result = collect_files_for_mod(&r#mod, &config, &mut groups).await;

        assert!(result.is_ok());
        assert!(groups.is_empty());
    }

    /// The Skip List marks patch names whose `.patch_0` is the game's own
    /// file (deploy starts those names at `.patch_1`). Purge, and so the
    /// purge every deploy starts with, must leave that slot 0 alone; every
    /// other patch file, including the skip-listed name's higher slots, goes.
    #[tokio::test]
    async fn purge_keeps_skip_listed_slot_zero_files() {
        let settings: Settings = serde_json::from_str(
            r#"{"Version":"V1","GamePath":"","SkipList":["0123456789abcdef"]}"#,
        )
        .unwrap();
        let data = tempfile::tempdir().unwrap();
        let kept = [
            "0123456789abcdef.patch_0",
            "0123456789abcdef.patch_0.gpu_resources",
            "0123456789abcdef.patch_0.stream",
            "game.bin",
        ];
        let purged = [
            "0123456789abcdef.patch_1",
            "0123456789abcdef.patch_1.stream",
            "0123456789abcdef.patch_10",
            "fedcba9876543210.patch_0",
            "fedcba9876543210.patch_0.gpu_resources",
        ];
        for name in kept.iter().chain(purged.iter()) {
            tokio::fs::write(data.path().join(name), b"x").await.unwrap();
        }

        do_purge(data.path(), &settings).await.unwrap();

        for name in kept {
            assert!(data.path().join(name).exists(), "{name} must be kept");
        }
        for name in purged {
            assert!(!data.path().join(name).exists(), "{name} must be purged");
        }
    }

    fn legacy_mod(guid: &str, dir: &std::path::Path) -> (Mod, Config) {
        let guid = Uuid::parse_str(guid).unwrap();
        let r#mod = Mod {
            manifest: Manifest::Legacy(legacy::Manifest {
                guid,
                name: guid.to_string(),
                description: String::new(),
                icon_path: None,
                options: None,
            }),
            directory: dir.to_path_buf(),
            sources: Vec::new(),
        };
        (r#mod, Config::Legacy { guid, enabled: true, selected: 0 })
    }

    fn skip_list(name: &str) -> Settings {
        serde_json::from_str(&format!(r#"{{"Version":"V1","GamePath":"","SkipList":["{name}"]}}"#)).unwrap()
    }

    /// A mod deployed into slot 0 of a name that was added to the Skip List
    /// only afterwards is DDMM's file, not the game's: the next purge must
    /// still remove it.
    #[tokio::test]
    async fn purge_removes_ddmm_slot_zero_files_of_a_name_skip_listed_later() {
        let data = tempfile::tempdir().unwrap();
        let m = tempfile::tempdir().unwrap();
        write_patch_file(m.path(), 0).await;
        let (r#mod, cfg) = legacy_mod("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", m.path());
        let mods = vec![r#mod];
        let configs = vec![cfg];

        do_deploy(data.path(), &no_skip_list(), pair_mods_with_configs(&mods, &configs)).await.unwrap();
        let slot0 = data.path().join(patch_file_name(0));
        assert!(slot0.exists());

        do_purge(data.path(), &skip_list("0123456789abcdef")).await.unwrap();
        assert!(!slot0.exists(), "DDMM's own slot-0 file must be purged");
        assert!(!data.path().join(format!("{}.stream", patch_file_name(0))).exists());
        assert!(!data.path().join(DEPLOY_RECORD).exists(), "purge removes the record");

        // A deploy with the name skip-listed starts at slot 1 and leaves a
        // game file in slot 0 alone, through the purge of the next deploy.
        tokio::fs::write(&slot0, b"game").await.unwrap();
        do_deploy(data.path(), &skip_list("0123456789abcdef"), pair_mods_with_configs(&mods, &configs)).await.unwrap();
        do_deploy(data.path(), &skip_list("0123456789abcdef"), pair_mods_with_configs(&mods, &configs)).await.unwrap();
        assert_eq!(tokio::fs::read(&slot0).await.unwrap(), b"game");
        assert_eq!(tokio::fs::read(data.path().join(patch_file_name(1))).await.unwrap(), b"data");
    }

    /// DDMM deployed into slot 0, the name was skip-listed afterwards, and
    /// then something else (a Steam "verify files") put the game's own
    /// file back. The record no longer matches it, so purge keeps it --
    /// whether its length or only its modification time changed.
    #[tokio::test]
    async fn purge_keeps_a_recorded_slot_zero_file_that_was_replaced_since() {
        for same_length in [false, true] {
            let data = tempfile::tempdir().unwrap();
            let m = tempfile::tempdir().unwrap();
            write_patch_file(m.path(), 0).await;
            let (r#mod, cfg) = legacy_mod("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", m.path());
            let mods = vec![r#mod];
            let configs = vec![cfg];
            do_deploy(data.path(), &no_skip_list(), pair_mods_with_configs(&mods, &configs)).await.unwrap();

            let slot0 = data.path().join(patch_file_name(0));
            let restored: &[u8] = if same_length { b"game" } else { b"the game's own file" };
            tokio::fs::write(&slot0, restored).await.unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&slot0).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap();
            drop(file);

            do_purge(data.path(), &skip_list("0123456789abcdef")).await.unwrap();
            assert_eq!(tokio::fs::read(&slot0).await.unwrap(), restored, "same length: {same_length}");
            // The untouched companions DDMM wrote still match: purged.
            assert!(!data.path().join(format!("{}.stream", patch_file_name(0))).exists());
        }
    }

    /// The deploy record can't be written (here a folder is in its way):
    /// the deploy fails with an error saying so before touching the game
    /// folder, so the previous deploy stays in place and no new file lands
    /// without a record.
    #[tokio::test]
    async fn a_deploy_record_that_cant_be_written_leaves_the_previous_deploy_in_place() {
        let f = Fixture::new().await;
        let settings = skip_lists(&[GAME_NAME]);
        f.deploy(&settings).await.unwrap();
        let before = patch_files_in(f.dir());
        std::fs::remove_file(f.dir().join(DEPLOY_RECORD)).unwrap();
        std::fs::create_dir(f.dir().join(DEPLOY_RECORD)).unwrap();

        let err = f.deploy(&settings).await.unwrap_err();
        assert!(format!("{err:#}").contains("still in place"), "{err:#}");
        assert_eq!(patch_files_in(f.dir()), before, "the previous deploy must be untouched");
        assert_eq!(std::fs::read(f.dir().join(format!("{MOD_NAME}.patch_0"))).unwrap(), b"mod data");
    }

    /// The game's data folder can't be written to at all: the deploy fails
    /// clearly and leaves the previous deploy in place.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_only_game_folder_leaves_the_previous_deploy_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new().await;
        let settings = skip_lists(&[GAME_NAME]);
        f.deploy(&settings).await.unwrap();
        let before = patch_files_in(f.dir());
        std::fs::set_permissions(f.dir(), std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::write(f.dir().join("probe"), b"x").is_ok() {
            // Running as root: permissions don't apply.
            std::fs::set_permissions(f.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let result = f.deploy(&settings).await;
        std::fs::set_permissions(f.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("still in place"), "{err:#}");
        assert_eq!(patch_files_in(f.dir()), before);
    }

    /// The probe write keeps the record exactly as it was.
    #[tokio::test]
    async fn the_record_probe_changes_nothing() {
        let data = tempfile::tempdir().unwrap();
        probe_deploy_record(data.path()).await.unwrap();
        let empty = std::fs::read(data.path().join(DEPLOY_RECORD)).unwrap();
        let record: DeployRecord = serde_json::from_slice(&empty).unwrap();
        assert!(record.files.is_empty() && record.pending.is_empty());

        let existing = br#"{"Files":[{"Name":"0123456789abcdef.patch_0","Len":4,"Modified":5}]}"#;
        std::fs::write(data.path().join(DEPLOY_RECORD), existing).unwrap();
        probe_deploy_record(data.path()).await.unwrap();
        assert_eq!(std::fs::read(data.path().join(DEPLOY_RECORD)).unwrap(), existing);
        assert_eq!(patch_files_in(data.path()), vec![DEPLOY_RECORD.to_string()], "no temporary file left");
    }

    /// The record can be written before the purge but not after it: the
    /// deploy stops before copying, saying so, and the game has no mods.
    #[tokio::test]
    async fn a_pending_record_that_cant_be_written_stops_the_deploy_before_copying() {
        let f = Fixture::new().await;
        let settings = skip_lists(&[GAME_NAME]);
        f.deploy(&settings).await.unwrap();
        let err = crashing_at(DeployCrashPoint::PendingRecordFails, f.deploy(&settings)).await.unwrap_err();
        assert!(format!("{err:#}").contains("nothing was deployed"), "{err:#}");
        f.assert_only_game_files("nothing copied after the purge");
    }

    /// The final record can't be written: the deploy reports it, and the
    /// pending record still lets purge remove everything.
    #[tokio::test]
    async fn a_final_record_that_cant_be_written_is_reported() {
        let f = Fixture::new().await;
        let err = crashing_at(DeployCrashPoint::FinalRecordFails, f.deploy(&skip_lists(&[GAME_NAME]))).await.unwrap_err();
        assert!(format!("{err:#}").contains("the mods were deployed"), "{err:#}");
        assert_eq!(std::fs::read(f.dir().join(format!("{MOD_NAME}.patch_0"))).unwrap(), b"mod data");
        let record: DeployRecord =
            serde_json::from_slice(&std::fs::read(f.dir().join(DEPLOY_RECORD)).unwrap()).unwrap();
        assert_eq!(record.pending.len(), 6);

        do_purge(f.dir(), &skip_lists(&[GAME_NAME, MOD_NAME])).await.unwrap();
        f.assert_only_game_files("purged with the pending record");
    }

    /// Purge removes the temporary record files an interrupted record
    /// write leaves, and nothing merely named like them.
    #[tokio::test]
    async fn purge_removes_leftover_temporary_record_files() {
        let data = tempfile::tempdir().unwrap();
        let leftover = format!("{DEPLOY_RECORD}.{}.tmp", uuid::Uuid::new_v4());
        let others = [format!("{DEPLOY_RECORD}.notes.tmp"), format!("{DEPLOY_RECORD}.bak"), "other.tmp".to_string()];
        std::fs::write(data.path().join(&leftover), b"{").unwrap();
        for name in &others {
            std::fs::write(data.path().join(name), b"x").unwrap();
        }
        do_purge(data.path(), &no_skip_list()).await.unwrap();
        assert!(!data.path().join(&leftover).exists());
        for name in &others {
            assert!(data.path().join(name).exists(), "{name} must be kept");
        }
    }

    /// The names of the patch files (and record leftovers) in `dir`, sorted.
    fn patch_files_in(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| is_patch_filename(n) || n.starts_with(DEPLOY_RECORD))
            .collect();
        names.sort();
        names
    }

    /// Run `f` as if DDMM crashed at `at` in it.
    async fn crashing_at<T>(at: DeployCrashPoint, f: impl std::future::Future<Output = T>) -> T {
        DEPLOY_CRASH_AT.with(|c| c.set(Some(at)));
        let out = f.await;
        DEPLOY_CRASH_AT.with(|c| c.set(None));
        out
    }

    const MOD_NAME: &str = "0123456789abcdef";
    /// A name whose slot 0 is the game's own file (skip-listed).
    const GAME_NAME: &str = "1111111111111111";

    fn skip_lists(names: &[&str]) -> Settings {
        let list = names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(",");
        serde_json::from_str(&format!(r#"{{"Version":"V1","GamePath":"","SkipList":[{list}]}}"#)).unwrap()
    }

    /// A game folder with the game's own files (a skip-listed slot 0
    /// triplet, and a non-patch file), and a mod with a full triplet for
    /// [`MOD_NAME`] and a patch for [`GAME_NAME`].
    struct Fixture {
        data: tempfile::TempDir,
        _mod_dir: tempfile::TempDir,
        mods: Vec<Mod>,
        configs: Vec<Config>,
    }

    const GAME_FILES: [&str; 4] = [
        "1111111111111111.patch_0",
        "1111111111111111.patch_0.gpu_resources",
        "1111111111111111.patch_0.stream",
        "game.bin",
    ];

    impl Fixture {
        async fn new() -> Self {
            let data = tempfile::tempdir().unwrap();
            for name in GAME_FILES {
                tokio::fs::write(data.path().join(name), format!("game's own {name}")).await.unwrap();
            }
            let mod_dir = tempfile::tempdir().unwrap();
            for suffix in ["", ".gpu_resources", ".stream"] {
                tokio::fs::write(mod_dir.path().join(format!("{MOD_NAME}.patch_0{suffix}")), b"mod data").await.unwrap();
            }
            tokio::fs::write(mod_dir.path().join(format!("{GAME_NAME}.patch_0")), b"mod data").await.unwrap();
            let (r#mod, cfg) = legacy_mod("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", mod_dir.path());
            Fixture { data, _mod_dir: mod_dir, mods: vec![r#mod], configs: vec![cfg] }
        }

        fn dir(&self) -> &std::path::Path {
            self.data.path()
        }

        async fn deploy(&self, settings: &Settings) -> anyhow::Result<()> {
            do_deploy(self.dir(), settings, pair_mods_with_configs(&self.mods, &self.configs)).await
        }

        /// Only the game's own files are left, unchanged.
        fn assert_only_game_files(&self, context: &str) {
            let mut left: Vec<String> = std::fs::read_dir(self.dir())
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            left.sort();
            assert_eq!(left, GAME_FILES, "{context}");
            for name in GAME_FILES {
                assert_eq!(
                    std::fs::read_to_string(self.dir().join(name)).unwrap(),
                    format!("game's own {name}"),
                    "{context}: {name} changed"
                );
            }
        }
    }

    const DEPLOY_CRASH_POINTS: [DeployCrashPoint; 5] = [
        DeployCrashPoint::PurgeBeforeRecordRemoved,
        DeployCrashPoint::BeforePendingRecord,
        DeployCrashPoint::BeforeCopy,
        DeployCrashPoint::AfterFirstFile,
        DeployCrashPoint::BeforeFinalRecord,
    ];

    /// After a deploy stopped at [`DeployCrashPoint::AfterFirstFile`]:
    /// the one file copied is the first by name, [`MOD_NAME`]'s slot 0
    /// (skip-listed in the tests' later purge). Make it a copy cut short,
    /// with the space it had reserved still zero.
    fn cut_short_first_copy(f: &Fixture, at: DeployCrashPoint) {
        if at != DeployCrashPoint::AfterFirstFile {
            return;
        }
        let mut expected = vec![format!("{MOD_NAME}.patch_0"), DEPLOY_RECORD.to_string()];
        expected.extend(GAME_FILES[..3].iter().map(|n| n.to_string()));
        expected.sort();
        assert_eq!(patch_files_in(f.dir()), expected, "the copy order is by name");
        std::fs::write(f.dir().join(format!("{MOD_NAME}.patch_0")), b"mod\0\0").unwrap();
    }

    /// A deploy stopped at any point, followed by a purge, leaves exactly
    /// the game's own files -- also when the mod's patch name was added to
    /// the Skip List in between (so its slot 0 is only purged if the record
    /// shows DDMM wrote it), and when a file was cut short mid-copy.
    #[tokio::test]
    async fn purge_after_a_deploy_stopped_at_any_point_removes_every_ddmm_file() {
        for at in DEPLOY_CRASH_POINTS {
            let f = Fixture::new().await;
            // A finished deploy first, so the stopped one has something to purge.
            f.deploy(&skip_lists(&[GAME_NAME])).await.unwrap();
            let result = crashing_at(at, f.deploy(&skip_lists(&[GAME_NAME]))).await;
            assert!(result.is_err(), "{at:?}");
            cut_short_first_copy(&f, at);

            do_purge(f.dir(), &skip_lists(&[GAME_NAME, MOD_NAME])).await.unwrap();
            f.assert_only_game_files(&format!("{at:?}"));
        }
    }

    /// The next deploy after a stopped one cleans up the same way, and
    /// deploys normally.
    #[tokio::test]
    async fn deploy_after_a_deploy_stopped_at_any_point_replaces_it() {
        for at in DEPLOY_CRASH_POINTS {
            let f = Fixture::new().await;
            f.deploy(&skip_lists(&[GAME_NAME])).await.unwrap();
            let _ = crashing_at(at, f.deploy(&skip_lists(&[GAME_NAME]))).await;
            cut_short_first_copy(&f, at);

            let both = skip_lists(&[GAME_NAME, MOD_NAME]);
            f.deploy(&both).await.unwrap();
            let mut expected: Vec<String> = [MOD_NAME, GAME_NAME]
                .iter()
                .flat_map(|n| ["", ".gpu_resources", ".stream"].map(|s| format!("{n}.patch_1{s}")))
                .chain(GAME_FILES[..3].iter().map(|n| n.to_string()))
                .chain([DEPLOY_RECORD.to_string()])
                .collect();
            expected.sort();
            assert_eq!(patch_files_in(f.dir()), expected, "{at:?}");

            do_purge(f.dir(), &both).await.unwrap();
            f.assert_only_game_files(&format!("{at:?}"));
        }
    }

    /// A purge that stops before removing the record is finished by the
    /// next one.
    #[tokio::test]
    async fn a_stopped_purge_is_finished_by_the_next() {
        let f = Fixture::new().await;
        f.deploy(&skip_lists(&[GAME_NAME])).await.unwrap();
        let both = skip_lists(&[GAME_NAME, MOD_NAME]);
        assert!(crashing_at(DeployCrashPoint::PurgeBeforeRecordRemoved, do_purge(f.dir(), &both)).await.is_err());
        do_purge(f.dir(), &both).await.unwrap();
        f.assert_only_game_files("after the second purge");
    }

    /// After a stopped deploy, something else (a Steam "verify files")
    /// put a game file in a slot the deploy had claimed, and that name is
    /// now skip-listed: a file whose bytes aren't DDMM's copy of the mod
    /// file is kept, whether longer or the same length. One with exactly
    /// the mod's bytes can't be told apart, and goes.
    #[tokio::test]
    async fn a_game_file_restored_after_a_stopped_deploy_is_kept() {
        for (restored, kept) in [
            (&b"the game's own, longer file"[..], true),
            (b"gamedata", true),
            (b"mod\0data", true),
            (b"mod data", false),
        ] {
            let f = Fixture::new().await;
            let _ = crashing_at(DeployCrashPoint::BeforeFinalRecord, f.deploy(&skip_lists(&[GAME_NAME]))).await;
            let slot0 = f.dir().join(format!("{MOD_NAME}.patch_0"));
            tokio::fs::write(&slot0, restored).await.unwrap();

            do_purge(f.dir(), &skip_lists(&[GAME_NAME, MOD_NAME])).await.unwrap();
            let context = String::from_utf8_lossy(restored).into_owned();
            assert_eq!(slot0.exists(), kept, "{context}");
            if kept {
                assert_eq!(tokio::fs::read(&slot0).await.unwrap(), restored, "{context}");
            }
            // DDMM's files around it are gone.
            assert!(!f.dir().join(format!("{MOD_NAME}.patch_0.stream")).exists());
            assert!(!f.dir().join(format!("{GAME_NAME}.patch_1")).exists());
        }
    }

    /// A pending entry whose mod file is gone (the mod was removed since)
    /// can't show the file is DDMM's: it is kept.
    #[tokio::test]
    async fn a_pending_entry_whose_mod_file_is_gone_keeps_the_file() {
        let f = Fixture::new().await;
        let _ = crashing_at(DeployCrashPoint::BeforeFinalRecord, f.deploy(&skip_lists(&[GAME_NAME]))).await;
        std::fs::remove_file(f._mod_dir.path().join(format!("{MOD_NAME}.patch_0"))).unwrap();

        do_purge(f.dir(), &skip_lists(&[GAME_NAME, MOD_NAME])).await.unwrap();
        assert!(f.dir().join(format!("{MOD_NAME}.patch_0")).exists());
        // Its companions, whose mod files are still there, go.
        assert!(!f.dir().join(format!("{MOD_NAME}.patch_0.stream")).exists());
    }

    /// The pending-record rule: a file DDMM was about to write is DDMM's
    /// only if nothing of that name was there before, it isn't longer than
    /// DDMM's copy, and its bytes are the mod file's (possibly cut short,
    /// then zeros).
    #[tokio::test]
    async fn pending_entries_claim_only_ddmm_copies_that_werent_there_before() {
        let data = tempfile::tempdir().unwrap();
        let src = data.path().join("source");
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8 + 1).collect();
        std::fs::write(&src, &big).unwrap();
        let file = data.path().join(patch_file_name(0));
        let pending = |len: u64, source: Option<PathBuf>, existed| Deployed {
            files: HashMap::new(),
            pending: HashMap::from([(patch_file_name(0), PendingFile { name: patch_file_name(0), len, source, existed })]),
        };
        let len = big.len() as u64;
        let with = |bytes: &[u8]| std::fs::write(&file, bytes).unwrap();

        with(&big);
        assert!(pending(len, Some(src.clone()), false).wrote(&file).await, "the whole copy");
        assert!(!pending(len, Some(src.clone()), true).wrote(&file).await, "it was there before");
        assert!(!pending(len - 1, Some(src.clone()), false).wrote(&file).await, "longer than DDMM's copy");
        with(&big[..100_000]);
        assert!(pending(len, Some(src.clone()), false).wrote(&file).await, "cut short");
        let mut reserved = big[..70_000].to_vec();
        reserved.resize(big.len(), 0);
        with(&reserved);
        assert!(pending(len, Some(src.clone()), false).wrote(&file).await, "cut short, reserved space zero");
        let mut changed = big.clone();
        changed[150_000] ^= 0xff;
        with(&changed);
        assert!(!pending(len, Some(src.clone()), false).wrote(&file).await, "one byte differs");
        let mut tail = reserved.clone();
        tail[190_000] = 7;
        with(&tail);
        assert!(!pending(len, Some(src.clone()), false).wrote(&file).await, "not zeros after the cut");
        with(&big);
        assert!(!pending(len, Some(data.path().join("gone")), false).wrote(&file).await, "mod file gone");

        with(b"");
        assert!(pending(0, None, false).wrote(&file).await, "an empty placeholder");
        with(b"x");
        assert!(!pending(1, None, false).wrote(&file).await, "a placeholder is empty");
        assert!(!Deployed::default().wrote(&file).await);
        assert!(!pending(1, None, false).wrote(&data.path().join(patch_file_name(1))).await);
    }

    /// Deploy and purge cycles, with stopped deploys in between, keep
    /// working and always end with only the game's files.
    #[tokio::test]
    async fn repeated_deploy_and_purge_cycles() {
        let f = Fixture::new().await;
        let settings = skip_lists(&[GAME_NAME]);
        for round in 0..3 {
            f.deploy(&settings).await.unwrap();
            f.deploy(&settings).await.unwrap();
            assert_eq!(
                std::fs::read(f.dir().join(format!("{MOD_NAME}.patch_0"))).unwrap(),
                b"mod data",
                "round {round}"
            );
            let _ = crashing_at(DEPLOY_CRASH_POINTS[round + 1], f.deploy(&settings)).await;
            f.deploy(&settings).await.unwrap();
            do_purge(f.dir(), &settings).await.unwrap();
            f.assert_only_game_files(&format!("round {round}"));
            do_purge(f.dir(), &settings).await.unwrap();
            f.assert_only_game_files(&format!("round {round}, purged twice"));
        }
    }

    /// A finished deploy leaves a final record (no pending entries) with
    /// every file it wrote.
    #[tokio::test]
    async fn a_finished_deploy_leaves_a_final_record() {
        let f = Fixture::new().await;
        f.deploy(&skip_lists(&[GAME_NAME])).await.unwrap();
        let record: DeployRecord =
            serde_json::from_slice(&std::fs::read(f.dir().join(DEPLOY_RECORD)).unwrap()).unwrap();
        assert!(record.pending.is_empty());
        let mut names: Vec<&str> = record.files.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        assert_eq!(names.len(), 6, "{names:?}");
        let raw = std::fs::read_to_string(f.dir().join(DEPLOY_RECORD)).unwrap();
        assert!(!raw.contains("Pending"), "{raw}");
    }

    /// No record (a game folder last deployed by an older version) or a
    /// damaged one: a skip-listed slot-0 file can't be shown to be DDMM's,
    /// so it is kept. Everything else is still purged.
    #[tokio::test]
    async fn without_a_usable_record_skip_listed_slot_zero_is_kept() {
        for record in [None, Some(&b"{not json"[..]), Some(&b"{\"Files\": 7}"[..])] {
            let data = tempfile::tempdir().unwrap();
            let slot0 = data.path().join(patch_file_name(0));
            let slot1 = data.path().join(patch_file_name(1));
            tokio::fs::write(&slot0, b"x").await.unwrap();
            tokio::fs::write(&slot1, b"x").await.unwrap();
            if let Some(record) = record {
                tokio::fs::write(data.path().join(DEPLOY_RECORD), record).await.unwrap();
            }

            do_purge(data.path(), &skip_list("0123456789abcdef")).await.unwrap();
            assert!(slot0.exists(), "record {record:?}: slot 0 must be kept");
            assert!(!slot1.exists(), "record {record:?}: slot 1 must be purged");
            assert!(!data.path().join(DEPLOY_RECORD).exists());
        }
    }

    fn no_skip_list() -> Settings {
        serde_json::from_str(r#"{"Version":"V1","GamePath":"","SkipList":[]}"#).unwrap()
    }

    /// A mod that can't be deployed (here: its selected option's folder is
    /// missing) must fail the deploy before the game folder is purged, so
    /// the mods deployed last time stay in the game.
    #[tokio::test]
    async fn a_failing_deploy_leaves_the_previous_deploy_in_place() {
        let data = tempfile::tempdir().unwrap();
        let previous = data.path().join("fedcba9876543210.patch_0");
        tokio::fs::write(&previous, b"deployed last time").await.unwrap();

        let good = tempfile::tempdir().unwrap();
        write_patch_file(good.path(), 0).await;
        let (good_mod, good_cfg) = legacy_mod("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", good.path());
        let broken = tempfile::tempdir().unwrap();
        let guid = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
        let broken_mod = Mod {
            manifest: Manifest::Legacy(legacy::Manifest {
                guid,
                name: "broken".into(),
                description: String::new(),
                icon_path: None,
                options: Some(vec!["NotThere".into()]),
            }),
            directory: broken.path().to_path_buf(),
            sources: Vec::new(),
        };
        let broken_cfg = Config::Legacy { guid, enabled: true, selected: 0 };

        let mods = vec![good_mod, broken_mod];
        let configs = vec![good_cfg, broken_cfg];
        let err = do_deploy(data.path(), &no_skip_list(), pair_mods_with_configs(&mods, &configs))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("NotThere"), "{err:#}");
        assert!(previous.exists(), "the previous deploy must not be purged by a deploy that fails");
        assert!(!data.path().join(patch_file_name(0)).exists());

        // Without the broken mod the deploy goes through, replacing the old files.
        let ok = do_deploy(data.path(), &no_skip_list(), pair_mods_with_configs(&mods, &configs[..1])).await;
        ok.unwrap();
        assert!(!previous.exists());
        assert_eq!(tokio::fs::read(data.path().join(patch_file_name(0))).await.unwrap(), b"data");
        assert!(data.path().join(format!("{}.gpu_resources", patch_file_name(0))).exists());
        assert!(data.path().join(format!("{}.stream", patch_file_name(0))).exists());
    }

    /// The deploy index of a file is its position in its group, so this pins
    /// down the load order: list order top to bottom, and a mod's own
    /// patch files in ascending order.
    #[tokio::test]
    async fn load_order_follows_list_order_then_each_mods_own_patch_order() {
        let top = tempfile::tempdir().unwrap();
        let bottom = tempfile::tempdir().unwrap();
        for i in [3, 0, 2, 1] {
            write_patch_file(top.path(), i).await;
        }
        write_patch_file(bottom.path(), 0).await;
        let (top_mod, top_cfg) = legacy_mod("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", top.path());
        let (bottom_mod, bottom_cfg) = legacy_mod("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", bottom.path());

        let mods = vec![bottom_mod, top_mod];
        // The profile list order, top first (not the installed-mods order).
        let configs = vec![top_cfg, bottom_cfg];
        let mut groups = HashMap::new();
        for (r#mod, config) in pair_mods_with_configs(&mods, &configs) {
            collect_files_for_mod(r#mod, config, &mut groups).await.unwrap();
        }

        let order: Vec<PathBuf> = groups["0123456789abcdef"]
            .iter()
            .map(|t| t.patch.clone().unwrap())
            .collect();
        let expected: Vec<PathBuf> = (0..4)
            .map(|i| top.path().join(patch_file_name(i)))
            .chain(std::iter::once(bottom.path().join(patch_file_name(0))))
            .collect();
        assert_eq!(order, expected, "bottom mod must get the highest patch index");
    }
}
