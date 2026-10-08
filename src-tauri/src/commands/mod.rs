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
const DEPLOY_RECORD: &str = ".ddmm-deployed.json";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DeployRecord {
    files: Vec<String>,
}

/// The file names the last deploy recorded writing, or `None` when there
/// is no usable record (none yet -- including a game folder last deployed
/// by a version without records -- or an unreadable or damaged one).
async fn read_deploy_record(data_dir: &Path) -> Option<HashSet<String>> {
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
        Ok(record) => Some(record.files.into_iter().filter(|f| is_patch_filename(f)).collect()),
        Err(e) => {
            log::warn!("Ignoring the damaged deploy record {:?}: {}", file, e);
            None
        }
    }
}

/// Record `files` (names in `data_dir`) as written by DDMM, before writing
/// them, so a deploy that is interrupted is still on record.
async fn write_deploy_record(data_dir: &Path, files: Vec<String>) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(&DeployRecord { files })?;
    crate::fs_util::replace_file(&data_dir.join(DEPLOY_RECORD), &data).await
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

async fn do_purge(data_dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    use anyhow::Context;
    log::info!("Purging...");

    let deployed = read_deploy_record(data_dir).await;
    let mut patch_files = get_patch_files_from_dir(data_dir).await?;
    patch_files.retain(|f| {
        if !is_skip_listed_slot_zero(f, settings) {
            return true;
        }
        let ours = f
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| deployed.as_ref().is_some_and(|d| d.contains(n)));
        if !ours {
            log::info!("Keeping {:?}: its patch name is in the Skip List and DDMM didn't deploy it.", f);
        }
        ours
    });

    log::info!("Deleting files...");
    futures::future::try_join_all(patch_files.iter().map(|f| async move {
        tokio::fs::remove_file(f)
            .await
            .with_context(|| format!("failed to delete {:?}", f))
    }))
    .await?;

    // Everything DDMM deployed is gone, so the record is too.
    let record = data_dir.join(DEPLOY_RECORD);
    match tokio::fs::remove_file(&record).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("failed to delete {:?}", record)),
    }

    log::info!("Purge complete.");
    Ok(())
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
/// patch files first, then purge, then copy. Collecting first means a
/// mod that can't be deployed (an option folder missing from it, an
/// unreadable folder) stops the deploy before anything in the game folder
/// is touched, so the previous deploy stays in place instead of the game
/// being left with no mods at all.
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

    do_purge(data_dir, settings).await?;

    log::info!("Deploying...");

    if groups.is_empty() {
        log::info!("Nothing to deploy.");
        return Ok(());
    }
    
    // Load order: `configs` is the profile's mod list, top to bottom, and
    // each group's triplets were collected in that order. The first gets
    // `<name>.patch_0` (after an optional skip-list offset), the next
    // `.patch_1`, and so on. Helldivers 2 applies higher patch numbers over
    // lower ones, so the mod LOWEST in the list wins a conflict.
    let mut written = Vec::new();
    for (name, triplets) in &groups {
        let offset = if settings.has_skip_entry(name) { 1 } else { 0 };
        for index in offset..triplets.len() + offset {
            for suffix in ["", ".gpu_resources", ".stream"] {
                written.push(format!("{}.patch_{}{}", name, index, suffix));
            }
        }
    }
    written.sort();
    write_deploy_record(data_dir, written).await?;

    log::info!("Copying files...");
    for (name, triplets) in &groups {
        let offset = if settings.has_skip_entry(name) { 1 } else { 0 };

        for (i, triplet) in triplets.iter().enumerate() {
            let index = i + offset;

            let patch_dest = data_dir.join(format!("{}.patch_{}", name, index));
            match &triplet.patch {
                Some(src) => { copy_patch_file(src, &patch_dest).await?; }
                None => { create_empty_patch_file(&patch_dest).await?; }
            }
            
            let gpu_dest = data_dir.join(format!("{}.patch_{}.gpu_resources", name, index));
            match &triplet.gpu_resources {
                Some(src) => { copy_patch_file(src, &gpu_dest).await?; }
                None => { create_empty_patch_file(&gpu_dest).await?; }
            }
            
            let stream_dest = data_dir.join(format!("{}.patch_{}.stream", name, index));
            match &triplet.stream {
                Some(src) => { copy_patch_file(src, &stream_dest).await?; }
                None => { create_empty_patch_file(&stream_dest).await?; }
            }
        }
    }

    log::info!("Deployment complete.");
    Ok(())
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
