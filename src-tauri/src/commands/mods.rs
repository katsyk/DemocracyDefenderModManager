use crate::{
    archive::Archive,
    download,
    models::{
        manifest::{legacy, Manifest, Source},
        Mod,
    },
    sources,
    utils::copy_dir_recursive,
    AppState,
};
use anyhow_tauri::{IntoTAResult, TAResult};
use rand::{rngs::SysRng, TryRng};
use std::{collections::HashSet, path::{Path, PathBuf}};
use tauri::State;
use uuid::Uuid;

const MODS_DIRECTORY: &'static str = "mods/";
const MANIFEST_FILE: &'static str = "manifest.json";

#[allow(dead_code)]
trait ZipResult<S, E, T> {
    fn zip(self, other: Result<T, E>) -> Result<(S, T), E>;
    fn zip_value(self, other: T) -> Result<(S, T), E>;
}

impl<S, E, T> ZipResult<S, E, T> for Result<S, E> {
    fn zip(self, other: Result<T, E>) -> Result<(S, T), E> {
        match (self, other) {
            (Ok(a), Ok(b)) => Ok((a, b)),
            (Err(e), Ok(_)) => Err(e),
            (Ok(_), Err(e)) => Err(e),
            (Err(e), Err(_)) => Err(e),
        }
    }

    fn zip_value(self, other: T) -> Result<(S, T), E> {
        match self {
            Ok(s) => Ok((s, other)),
            Err(e) => Err(e)
        }
    }
}

#[tauri::command]
pub async fn get_mods(state: State<'_, AppState>) -> TAResult<Vec<Mod>> {
    let mut state_mods = state.mods.lock().await;

    log::info!("Loading mods...");

    if let Some(mods) = state_mods.as_ref() {
        log::info!("Mods already loaded.");
        return Ok(mods.clone());
    }

    let mods_dir = state.base_path.join(MODS_DIRECTORY);
    if !mods_dir.is_dir() {
        tokio::fs::create_dir(mods_dir).await.into_ta_result()?;
        return Ok(vec![]);
    }

    let mut mods = Vec::new();
    let mut mods_dir = tokio::fs::read_dir(mods_dir).await.into_ta_result()?;
    while let Some(entry) = mods_dir.next_entry().await.into_ta_result()? {
        let mod_dir = entry.path();

        let manifest_file = mod_dir.join(MANIFEST_FILE);
        if !manifest_file.is_file() {
            continue;
        }

        let manifest_data = tokio::fs::read(manifest_file).await.into_ta_result()?;
        let manifest: Manifest = serde_json::from_slice(&manifest_data).into_ta_result()?;

        let mut r#mod = Mod {
            manifest,
            directory: mod_dir,
            sources: Vec::new(),
        };
        r#mod.normalize_paths().await?;
        r#mod.resolve_sources().await;

        mods.push(r#mod);
    }

    *state_mods = Some(mods.clone());
    log::info!("Mods loaded.");
    Ok(mods)
}

#[tauri::command]
pub async fn delete_mod(state: State<'_, AppState>, guid: Uuid) -> TAResult<()> {
    let mut mods = state.mods.lock().await;
    if mods.is_none() {
        return anyhow::anyhow!("mods not read").into_ta_result();
    }
    let mods = mods.as_mut().unwrap();

    log::info!("Deleting mod \"{}\"...", guid);

    if let Some(i) = mods.iter().position(|m| m.guid() == guid) {
        let r#mod = mods.remove(i);
        log::info!("Mod removed form registry.");

        log::info!("Deleting files...");
        tokio::fs::remove_dir_all(r#mod.directory).await.into_ta_result()?;
        
        log::info!("Mod deletion complete.");
        Ok(())
    } else {
        anyhow_tauri::bail!("mod with GUID {{{}}} not found", guid);
    }
}

#[tauri::command]
pub async fn add_mod(state: State<'_, AppState>, archive_file: PathBuf) -> TAResult<Mod> {
    let mut mods = state.mods.lock().await;
    if mods.is_none() {
        return anyhow::anyhow!("mods not read").into_ta_result();
    }
    let mods = mods.as_mut().unwrap();

    install_from_archive(&state, mods, &archive_file).await
}

/// Shared core of `add_mod`: prepares the mod directory, resolves the
/// manifest (from the archive if present, generating a local one
/// otherwise), extracts, and registers the mod.
///
/// Takes the already-locked mods vector rather than `State` directly so it
/// can be called both from a `#[tauri::command]` that holds the lock for a
/// single install, and from callers (like `add_paths`) that lock once per
/// item without ever calling back into another `#[tauri::command]` while
/// holding the mutex.
async fn install_from_archive(state: &AppState, mods: &mut Vec<Mod>, archive_file: &Path) -> TAResult<Mod> {
    log::info!("Adding mod from {:?}...", archive_file);

    log::debug!("Opening archive...");
    let archive = Archive::open(archive_file)?;

    log::debug!("Obtaining name...");
    let name = archive_file
        .file_prefix()
        .unwrap()
        .to_str()
        .map(str::to_string)
        .ok_or(anyhow::anyhow!("file name conversion failed"))?;

    log::info!("Resolving mod directory...");
    let mut mod_dir = state.base_path.join(MODS_DIRECTORY);
    mod_dir.push(&name);

    log::info!("Preparing mod directory...");
    let manifest_file = mod_dir.join(MANIFEST_FILE);
    prepare_mod_dir(mod_dir.clone(), manifest_file.clone(), name.clone()).await.into_ta_result()?;

    log::info!("Resolving manifest...");
    let (archive, manifest) = resolve_manifest(archive, name.clone(), manifest_file.clone()).await.into_ta_result()?;

    let mut r#mod = Mod {
        manifest,
        directory: mod_dir.clone(),
        sources: Vec::new(),
    };

    log::info!("Checking for duplicate...");
    if mods.iter().any(|m| m.guid() == r#mod.guid()) {
        return anyhow::anyhow!("mod with GUID {{{}}} already exists", r#mod.guid())
            .into_ta_result();
    }

    log::info!("Extracting archive...");
    extract_archive(archive, mod_dir).await?;

    log::debug!("Normalizing paths...");
    if let Err(e) = r#mod.normalize_paths().await {
        log::error!("Path normalization failed: {}", e);
    }

    r#mod.resolve_sources().await;

    mods.push(r#mod.clone());
    log::info!("Mod successfully added.");
    Ok(r#mod)
}

#[tauri::command]
pub async fn add_mods(state: State<'_, AppState>, archive_files: Vec<PathBuf>) -> TAResult<Vec<TAResult<Mod>>> {
    let mut mods = state.mods.lock().await;
    if mods.is_none() {
        return anyhow::anyhow!("mods not read").into_ta_result();
    }
    let mods = mods.as_mut().unwrap();

    log::info!("Adding mods from:",);
    for archive_file in &archive_files {
        log::info!(" - {:?}", archive_file);
    }
    
    log::debug!("Opening archives...");
    let data = archive_files
        .iter()
        .map(|archive_file| Archive::open(archive_file).into_ta_result())
        .collect::<Vec<_>>();

    log::debug!("Obtaining names...");
    let data = archive_files
        .iter()
        .cloned()
        .zip(data)
        .map(|(archive_file, result)| {
            result
                .zip_value(archive_file)
                .map(|(archive, archive_file)| {
                    let name = archive_file
                        .file_prefix()
                        .unwrap()
                        .to_str()
                        .map(str::to_string)
                        .ok_or(anyhow::anyhow!("file name conversion failed"))
                        .into_ta_result()?;
                    Ok((archive, name))
                })
                .flatten()
        })
        .collect::<Vec<_>>();

    log::info!("Resolving mod directories...");
    let data = data
        .into_iter()
        .map(|result| {
            result
                .map(|(archive, name)| {
                    let mut mod_dir = state.base_path.join(MODS_DIRECTORY);
                    mod_dir.push(&name);
                    let manifest_file = mod_dir.join(MANIFEST_FILE);
                    (archive, name, mod_dir, manifest_file)
                })
        })
        .collect::<Vec<_>>();

    log::info!("Preparing mod directories...");
    let data = futures::future::join_all(
        data.into_iter().map(|result| async {
            match result {
                Ok((archive, name, mod_dir, manifest_file)) => {
                    prepare_mod_dir(mod_dir.clone(), manifest_file.clone(), name.clone()).await?;
                    Ok((archive, name, mod_dir, manifest_file))
                }
                Err(e) => Err(e)
            }
        })
    ).await;

    log::info!("Resolving manifests...");
    let data = futures::future::join_all(
        data.into_iter().map(|result| async {
            match result {
                Ok((archive, name, mod_dir, manifest_file)) => {
                    let (archive, manifest) = resolve_manifest(archive, name, manifest_file).await?;
                    Ok((archive, mod_dir, manifest))
                }
                Err(e) => Err(e)
            }
        })
    ).await;
    
    let data = data
        .into_iter()
        .map(|result| {
            result.map(|(archive, mod_dir, manifest)| {
                let r#mod = Mod {
                    manifest,
                    directory: mod_dir,
                    sources: Vec::new(),
                };
                (archive, r#mod)
            })
        })
        .collect::<Vec<_>>();
    
    log::info!("Checking for duplicates...");
    let mut guids: HashSet<Uuid> = mods.iter().map(|m| m.guid()).collect();
    let data = data
        .into_iter()
        .map(|result| {
            result.map(|(archive, r#mod)| {
                let guid = r#mod.guid();
                if guids.insert(guid) {
                    Ok((archive, r#mod))
                } else {
                    anyhow_tauri::bail!("mod with GUID {{{}}} already exists", guid)
                }
            })
            .flatten()
        })
        .collect::<Vec<_>>();

    let mut guids = HashSet::<Uuid>::new();
    let data = data
        .into_iter()
        .map(|result| {
            result.map(|(archive, r#mod)| {
                let guid = r#mod.guid();
                if guids.insert(guid) {
                    Ok((archive, r#mod))
                } else {
                    anyhow_tauri::bail!("already adding mod with GUID {{{}}}", guid)
                }
            })
            .flatten()
        })
        .collect::<Vec<_>>();

    log::info!("Extracting archives...");
    let data = futures::future::join_all(
        data.into_iter().map(|result| async {
            match result {
                Ok((archive, r#mod)) => {
                    extract_archive(archive, r#mod.directory.clone()).await?;
                    Ok(r#mod)
                }
                Err(e) => Err(e)
            }
        })
    ).await;

    log::debug!("Normalizing paths...");
    let data = futures::future::join_all(
        data.into_iter().map(|result| async {
            match result {
                Ok(mut r#mod) => {
                    if let Err(e) = r#mod.normalize_paths().await {
                        log::error!("Path normalization failed for \"{}\": {}", r#mod.guid(), e);
                    }
                    r#mod.resolve_sources().await;
                    Ok(r#mod)
                }
                Err(e) => Err(e)
            }
        })
    ).await;

    for r#mod in data.iter().flatten() {
        mods.push(r#mod.clone());
    }
    
    log::info!("Adding complete.");
    for (p, r) in archive_files.iter().zip(&data) {
        if let Err(e) = r {
            log::info!(" - {:?} : Err -> {}", p, e);
        } else {
            log::info!(" - {:?} : Ok", p);
        }
    }

    Ok(data)
}

async fn prepare_mod_dir(mod_dir: PathBuf, manifest_file: PathBuf, name: String) -> TAResult<()> {
    if tokio::fs::try_exists(&mod_dir).await.into_ta_result()? {
        if tokio::fs::try_exists(&manifest_file).await.into_ta_result()? {
            return anyhow::anyhow!("mod directory \"{}\" already exists", name).into_ta_result();
        } else {
            tokio::fs::remove_dir_all(&mod_dir).await.into_ta_result()?;
        }
    }
    tokio::fs::create_dir_all(&mod_dir).await.into_ta_result()
}

/// Generate the same "no manifest present" local legacy manifest, whether
/// the mod came from an archive with no `manifest.json`, or a plain folder
/// with no `manifest.json`.
fn generate_local_manifest(name: String) -> anyhow::Result<Manifest> {
    let mut guid = [0u8; 16];
    guid[..5].copy_from_slice(b"LOCAL");
    SysRng.try_fill_bytes(&mut guid[5..])?;

    Ok(Manifest::Legacy(legacy::Manifest {
        guid: Uuid::from_bytes(guid),
        name,
        description: String::new(),
        icon_path: None,
        options: None,
    }))
}

async fn resolve_manifest(mut archive: Archive, name: String, manifest_file: PathBuf) -> TAResult<(Archive, Manifest)> {
    if archive.has_path(MANIFEST_FILE)? {
        let manifest_data = archive.read_path(MANIFEST_FILE)?;
        let manifest = serde_json::from_slice(&manifest_data).into_ta_result()?;
        Ok((archive, manifest))
    } else {
        let manifest = generate_local_manifest(name).into_ta_result()?;

        let manifest_data = serde_json::to_vec_pretty(&manifest).into_ta_result()?;
        tokio::fs::write(&manifest_file, manifest_data)
            .await
            .into_ta_result()?;

        Ok((archive, manifest))
    }
}

/// Same idea as [`resolve_manifest`], but for a plain folder install: read
/// `manifest.json` from the source folder if present, otherwise generate a
/// local one and write it straight into the (already-created) destination
/// mod directory.
async fn resolve_manifest_for_dir(source_dir: &Path, name: String, manifest_file: PathBuf) -> TAResult<Manifest> {
    let source_manifest = source_dir.join(MANIFEST_FILE);
    if tokio::fs::try_exists(&source_manifest).await.into_ta_result()? {
        let manifest_data = tokio::fs::read(&source_manifest).await.into_ta_result()?;
        let manifest: Manifest = serde_json::from_slice(&manifest_data).into_ta_result()?;
        Ok(manifest)
    } else {
        let manifest = generate_local_manifest(name).into_ta_result()?;

        let manifest_data = serde_json::to_vec_pretty(&manifest).into_ta_result()?;
        tokio::fs::write(&manifest_file, manifest_data)
            .await
            .into_ta_result()?;

        Ok(manifest)
    }
}

async fn extract_archive(mut archive: Archive, mod_dir: PathBuf) -> TAResult<()> {
    tokio::task::spawn_blocking(move || archive.extract_to(mod_dir).into_ta_result()).await.into_ta_result()?
}

/// Install a mod from a plain, already-unpacked folder: `folder` becomes the
/// mod's source, its `manifest.json` is used if present (otherwise a local
/// one is generated), and its contents are copied (not moved, symlinks not
/// followed) into the managed mod directory.
///
/// Mirrors [`install_from_archive`]'s locking contract: takes the
/// already-locked mods vector, never locks anything itself.
async fn install_from_folder(state: &AppState, mods: &mut Vec<Mod>, folder: &Path) -> TAResult<Mod> {
    log::info!("Adding mod from folder {:?}...", folder);

    if !folder.is_dir() {
        return anyhow::anyhow!("path is not a directory").into_ta_result();
    }

    let name = folder
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or(anyhow::anyhow!("folder name conversion failed"))?;

    log::info!("Resolving mod directory...");
    let mut mod_dir = state.base_path.join(MODS_DIRECTORY);
    mod_dir.push(&name);

    log::info!("Preparing mod directory...");
    let manifest_file = mod_dir.join(MANIFEST_FILE);
    prepare_mod_dir(mod_dir.clone(), manifest_file.clone(), name.clone()).await.into_ta_result()?;

    let result = install_from_folder_inner(mods, folder, &name, &mod_dir, &manifest_file).await;

    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&mod_dir).await;
    }

    result
}

async fn install_from_folder_inner(
    mods: &mut Vec<Mod>,
    folder: &Path,
    name: &str,
    mod_dir: &Path,
    manifest_file: &Path,
) -> TAResult<Mod> {
    log::info!("Resolving manifest...");
    let manifest = resolve_manifest_for_dir(folder, name.to_string(), manifest_file.to_path_buf()).await?;

    let mut r#mod = Mod {
        manifest,
        directory: mod_dir.to_path_buf(),
        sources: Vec::new(),
    };

    log::info!("Checking for duplicate...");
    if mods.iter().any(|m| m.guid() == r#mod.guid()) {
        return anyhow::anyhow!("mod with GUID {{{}}} already exists", r#mod.guid())
            .into_ta_result();
    }

    log::info!("Copying folder contents...");
    copy_dir_recursive(folder, mod_dir, &[sources::ORIGIN_SIDECAR_FILE])
        .await
        .into_ta_result()?;

    log::debug!("Normalizing paths...");
    if let Err(e) = r#mod.normalize_paths().await {
        log::error!("Path normalization failed: {}", e);
    }

    r#mod.resolve_sources().await;

    mods.push(r#mod.clone());
    log::info!("Mod successfully added.");
    Ok(r#mod)
}

#[tauri::command]
pub async fn add_mod_folder(state: State<'_, AppState>, folder: PathBuf) -> TAResult<Mod> {
    let mut mods = state.mods.lock().await;
    if mods.is_none() {
        return anyhow::anyhow!("mods not read").into_ta_result();
    }
    let mods = mods.as_mut().unwrap();

    install_from_folder(&state, mods, &folder).await
}

/// Install any mix of archive files and plain folders in one go, dispatching
/// each path to the right installer. Each path locks the mods mutex
/// independently, so one item's failure never blocks the rest and nothing
/// ever nests a lock inside another lock.
#[tauri::command]
pub async fn add_paths(state: State<'_, AppState>, paths: Vec<PathBuf>) -> TAResult<Vec<TAResult<Mod>>> {
    log::info!("Adding mods from {} path(s)...", paths.len());

    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        let is_dir = tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false);

        let result = if is_dir {
            let mut mods = state.mods.lock().await;
            match mods.as_mut() {
                Some(mods) => install_from_folder(&state, mods, &path).await,
                None => anyhow::anyhow!("mods not read").into_ta_result(),
            }
        } else {
            let mut mods = state.mods.lock().await;
            match mods.as_mut() {
                Some(mods) => install_from_archive(&state, mods, &path).await,
                None => anyhow::anyhow!("mods not read").into_ta_result(),
            }
        };

        results.push(result);
    }

    Ok(results)
}

/// Install a mod by downloading an archive from a direct `https://` URL
/// first (into a throwaway temp directory), then running it through the
/// same install path as a locally picked archive file. Records where the
/// mod came from in a `.hd2mm-origin.json` sidecar next to the mod, without
/// ever touching the author's own `manifest.json`.
#[tauri::command]
pub async fn add_mod_from_url(state: State<'_, AppState>, url: String) -> TAResult<Mod> {
    log::info!("Downloading mod from {}...", url);

    let downloaded = download::download_archive(&url).await.into_ta_result()?;

    let install_result = {
        let mut mods = state.mods.lock().await;
        match mods.as_mut() {
            Some(mods) => install_from_archive(&state, mods, &downloaded.path).await,
            None => anyhow::anyhow!("mods not read").into_ta_result(),
        }
    };

    // The staged download (and its throwaway parent directory) is no longer
    // needed either way -- the archive contents were copied out on success.
    let _ = tokio::fs::remove_dir_all(&downloaded.temp_dir).await;

    let mut r#mod = install_result?;

    log::info!("Recording install origin...");
    let origin_source = Source {
        provider: sources::provider_from_url(&url),
        id: None,
        url: Some(url),
        version: None,
    };
    if let Err(e) = sources::write_origin_sidecar(&r#mod.directory, vec![origin_source]).await {
        log::error!("Failed to write origin sidecar: {}", e);
    }

    r#mod.resolve_sources().await;

    {
        let mut mods = state.mods.lock().await;
        if let Some(mods) = mods.as_mut() {
            if let Some(existing) = mods.iter_mut().find(|m| m.guid() == r#mod.guid()) {
                *existing = r#mod.clone();
            }
        }
    }

    Ok(r#mod)
}