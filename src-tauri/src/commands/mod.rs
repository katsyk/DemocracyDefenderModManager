use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}, sync::OnceLock};

use anyhow_tauri::{IntoTAResult, TAResult};
use regex::Regex;
use tauri::State;

use crate::{AppState, commands::settings::{do_load_settings, load_settings}, models::{manifest::Manifest, profile::Config, Mod}, utils::is_patch_filename};

pub mod mods;
pub mod profiles;
pub mod settings;
pub mod handoff;
pub mod updates;
pub mod bridge;

static INDEX_REGEX: OnceLock<Regex> = OnceLock::new();

struct PatchFileTriplet {
    patch: Option<PathBuf>,
    gpu_resources: Option<PathBuf>,
    stream: Option<PathBuf>,
}

async fn get_patch_files_from_dir(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    log::info!("Collecting patch files of directory {:?}...", dir);

    let mut entries = Vec::new();
    let mut dir_reader = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = dir_reader.next_entry().await? {
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

async fn do_purge(data_dir: &Path) -> anyhow::Result<()> {
    log::info!("Purging...");

    let patch_files = get_patch_files_from_dir(data_dir).await?;

    log::info!("Deleting files...");
    futures::future::try_join_all(patch_files.iter().map(|f| tokio::fs::remove_file(f))).await?;

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
                    let dir = base.join(opt);
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
                            let dir = base.join(inc);
                            add_files_from_dir(&dir, groups).await?;
                        }
                    }

                    if let Some(sub_options) = opt.sub_options.as_ref() {
                        if let Some(idx) = selected.get(i).cloned() {
                            if let Some(sub) = sub_options.get(idx) {
                                for inc in &sub.include {
                                    let dir = base.join(inc);
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
                            let dir = base.join(inc);
                            add_files_from_dir(&dir, groups).await?;
                        }
                    }

                    if let Some(sub_options) = opt.sub_options.as_ref() {
                        if let Some(idx) = selected.get(i).cloned() {
                            if let Some(sub) = sub_options.get(idx) {
                                for inc in &sub.include {
                                    let dir = base.join(inc);
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
    let mods = state.inner().mods.lock().await;
    if mods.is_none() {
        return anyhow::anyhow!("mods not read").into_ta_result();
    }
    let mods = mods.as_ref().unwrap();

    let settings = do_load_settings(&state.base_path).await?;
    if let Err(e) = settings.validate().await {
        return anyhow::anyhow!("invalid settings: {}", e).into_ta_result();
    }

    let mods = pair_mods_with_configs(mods, &configs);

    let data_dir = settings.game_path().join("data");

    do_purge(&data_dir).await?;

    log::info!("Deploying...");

    if mods.is_empty() {
        log::info!("Nothing to deploy.");
        return Ok(());
    }

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
    
    // Load order: `configs` is the profile's mod list, top to bottom, and
    // each group's triplets were collected in that order. The first gets
    // `<name>.patch_0` (after an optional skip-list offset), the next
    // `.patch_1`, and so on. Helldivers 2 applies higher patch numbers over
    // lower ones, so the mod LOWEST in the list wins a conflict.
    log::info!("Copying files...");
    for (name, triplets) in &groups {
        let offset = if settings.has_skip_entry(name) { 1 } else { 0 };

        for (i, triplet) in triplets.iter().enumerate() {
            let index = i + offset;

            let patch_dest = data_dir.join(format!("{}.patch_{}", name, index));
            match &triplet.patch {
                Some(src) => { tokio::fs::copy(src, &patch_dest).await.into_ta_result()?; }
                None => { tokio::fs::File::create(&patch_dest).await.into_ta_result()?; }
            }
            
            let gpu_dest = data_dir.join(format!("{}.patch_{}.gpu_resources", name, index));
            match &triplet.gpu_resources {
                Some(src) => { tokio::fs::copy(src, &gpu_dest).await.into_ta_result()?; }
                None => { tokio::fs::File::create(&gpu_dest).await.into_ta_result()?; }
            }
            
            let stream_dest = data_dir.join(format!("{}.patch_{}.stream", name, index));
            match &triplet.stream {
                Some(src) => { tokio::fs::copy(src, &stream_dest).await.into_ta_result()?; }
                None => { tokio::fs::File::create(&stream_dest).await.into_ta_result()?; }
            }
        }
    }

    log::info!("Deployment complete.");
    Ok(())
}

#[tauri::command]
pub async fn purge(state: State<'_, AppState>) -> TAResult<()> {
    let settings = load_settings(state).await?;
    if let Err(e) = settings.validate().await {
        return anyhow::anyhow!("invalid settings: {}", e).into_ta_result();
    }

    let data_dir = settings.game_path().join("data");
    do_purge(&data_dir).await.into_ta_result()
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
