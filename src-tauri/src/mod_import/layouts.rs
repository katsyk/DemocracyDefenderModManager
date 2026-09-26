//! Other mod managers' own records (read-only): what they know about each
//! mod beyond its files -- its name, Nexus id, on/off state, load order and
//! chosen options. Formats and sources are in
//! `docs/development/importing-from-other-managers.md`; every parser here is
//! tolerant (unknown or missing fields are skipped, never an error), since
//! none of these formats is a published contract.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::Value;
use uuid::Uuid;

use super::{NexusRef, ProfileHint};
use crate::models::{
    manifest::{v1, Manifest},
    profile::{Config, ProfilesConfig},
    Version,
};

// ---------------------------------------------------------------------------
// HD2 Arsenal: `hd2a_data.json` in its data folder (`%LOCALAPPDATA%\hd2arsenal`
// on Windows, `~/.config/hd2arsenal` on Linux), mods unpacked in `mods\`.
// ---------------------------------------------------------------------------

pub const ARSENAL_DATA_FILE: &str = "hd2a_data.json";

#[derive(Debug, Clone, Default)]
pub struct ArsenalOption {
    pub name: String,
    pub description: String,
    pub include: Vec<String>,
    pub suboptions: Vec<ArsenalOption>,
}

#[derive(Debug, Clone)]
pub struct ArsenalMod {
    pub uuid: String,
    pub path: PathBuf,
    pub label: String,
    pub description: String,
    pub options: Vec<ArsenalOption>,
    pub nexus: Option<NexusRef>,
    pub profile: Option<ProfileHint>,
}

#[derive(Debug, Clone)]
pub struct ArsenalData {
    pub mods: Vec<ArsenalMod>,
    pub profile_name: Option<String>,
}

/// `hd2a_data.json` for a folder the user picked: the data folder itself,
/// or its `mods` folder.
pub fn find_arsenal_data(root: &Path) -> Option<PathBuf> {
    [Some(root), root.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join(ARSENAL_DATA_FILE))
        .find(|f| f.is_file())
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_options(v: Option<&Value>) -> Vec<ArsenalOption> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    Some(ArsenalOption {
                        name: str_field(o, "name")?,
                        description: str_field(o, "description").unwrap_or_default(),
                        include: o
                            .get("include")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                            .unwrap_or_default(),
                        suboptions: parse_options(o.get("suboptions")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_nexus(v: Option<&Value>) -> Option<NexusRef> {
    let v = v?;
    let mod_id = str_field(v, "modId")?;
    if mod_id.is_empty() || !mod_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // The upload time as Nexus reported it: seconds, or an ISO string.
    let uploaded_at = match v.get("updateTimestamp") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok().or_else(|| crate::providers::parse_iso_utc(s)),
        _ => None,
    };
    Some(NexusRef {
        mod_id,
        file_id: str_field(v, "fileId").filter(|s| !s.is_empty()),
        version: str_field(v, "version").filter(|s| !s.is_empty()),
        uploaded_at,
        file_name: None,
        upload_order: uploaded_at.unwrap_or(0),
    })
}

/// Arsenal's per-profile option state (`optionsConfig`, matched to the
/// library entry's options by name) as DDMM's toggled/selected lists.
fn options_state(options: &[ArsenalOption], config: Option<&Value>) -> (Vec<bool>, Vec<usize>) {
    let configs = config.and_then(Value::as_array);
    let mut toggled = Vec::with_capacity(options.len());
    let mut selected = Vec::with_capacity(options.len());
    for (i, opt) in options.iter().enumerate() {
        let c = configs.and_then(|a| a.iter().find(|c| str_field(c, "name").as_deref() == Some(opt.name.as_str())));
        // Arsenal's default for an option it has no state for: the first
        // one on.
        toggled.push(c.and_then(|c| c.get("enabled")).and_then(Value::as_bool).unwrap_or(i == 0));
        let sub = c
            .and_then(|c| c.get("suboptions"))
            .and_then(Value::as_array)
            .and_then(|subs| {
                opt.suboptions.iter().position(|s| {
                    subs.iter().any(|x| {
                        str_field(x, "name").as_deref() == Some(s.name.as_str())
                            && x.get("enabled").and_then(Value::as_bool) == Some(true)
                    })
                })
            })
            .unwrap_or(0);
        selected.push(sub);
    }
    (toggled, selected)
}

pub fn load_arsenal(file: &Path) -> anyhow::Result<ArsenalData> {
    let data: Value = serde_json::from_slice(&std::fs::read(file)?)?;
    let selected = str_field(&data, "selectedProfile").unwrap_or_else(|| "default".to_string());
    let profiles = data.get("modsList");
    let profile = profiles.and_then(|p| p.get(&selected)).or_else(|| {
        profiles.and_then(Value::as_object).and_then(|m| m.values().next())
    });
    let profile_mods: Vec<&Value> = profile
        .and_then(|p| p.get("mods"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|m| str_field(m, "type").as_deref() != Some("separator")).collect())
        .unwrap_or_default();
    let top_priority = data.get("setTopPriority").and_then(Value::as_bool).unwrap_or(false);

    // Library entries (current format), else the full mod objects older
    // versions kept inside each profile.
    let mut entries: Vec<&Value> = data.get("modsLibrary").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    if entries.is_empty() {
        entries = profiles
            .and_then(Value::as_object)
            .map(|m| {
                m.values()
                    .filter_map(|p| p.get("mods").and_then(Value::as_array))
                    .flatten()
                    .filter(|m| m.get("path").is_some())
                    .collect()
            })
            .unwrap_or_default();
    }

    let count = profile_mods.len();
    let mut seen = std::collections::HashSet::new();
    let mut mods = Vec::new();
    for entry in entries {
        let (Some(uuid), Some(path)) = (str_field(entry, "uuid"), str_field(entry, "path")) else { continue };
        if !seen.insert(uuid.clone()) {
            continue;
        }
        let options = parse_options(entry.get("options"));
        let position = profile_mods.iter().position(|m| str_field(m, "uuid").as_deref() == Some(uuid.as_str()));
        let profile = position.map(|i| {
            let m = profile_mods[i];
            let (toggled, selected) = options_state(&options, m.get("optionsConfig").or_else(|| m.get("options")));
            // With "top priority" on, Arsenal deploys the list bottom-up.
            let order = if top_priority { count - 1 - i } else { i };
            ProfileHint {
                enabled: m.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                order,
                toggled: (!options.is_empty()).then_some(toggled),
                selected: (!options.is_empty()).then_some(selected),
            }
        });
        mods.push(ArsenalMod {
            label: str_field(entry, "label").unwrap_or_default(),
            description: str_field(entry, "description").unwrap_or_default(),
            nexus: parse_nexus(entry.get("nexusData")),
            path: PathBuf::from(path),
            uuid,
            options,
            profile,
        });
    }
    let profile_name = profile.and_then(|p| str_field(p, "label")).or(Some(selected));
    Ok(ArsenalData { mods, profile_name })
}

/// The manifest to give an imported Arsenal mod that has none of its own:
/// Arsenal's name, description and options (without its icons, which live
/// in Arsenal's own cache). `None` when there are no options, so DDMM's
/// usual auto-detection applies instead.
pub fn arsenal_manifest(m: &ArsenalMod) -> Option<Manifest> {
    if m.options.is_empty() {
        return None;
    }
    let guid = Uuid::parse_str(&m.uuid).unwrap_or_else(|_| Uuid::new_v4());
    let options = m
        .options
        .iter()
        .map(|o| v1::Option {
            name: o.name.clone(),
            description: o.description.clone(),
            include: (!o.include.is_empty()).then(|| o.include.iter().map(PathBuf::from).collect()),
            image: None,
            sub_options: (!o.suboptions.is_empty()).then(|| {
                o.suboptions
                    .iter()
                    .map(|s| v1::SubOption {
                        name: s.name.clone(),
                        description: s.description.clone(),
                        include: s.include.iter().map(PathBuf::from).collect(),
                        image: None,
                    })
                    .collect()
            }),
        })
        .collect();
    Some(Manifest::V1(v1::Manifest {
        version: Version::<1>,
        guid,
        name: if m.label.trim().is_empty() { "Imported mod".into() } else { m.label.trim().to_string() },
        description: m.description.clone(),
        icon_path: None,
        options: Some(options),
        nexus_data: None,
        sources: None,
    }))
}

// ---------------------------------------------------------------------------
// Helldivers 2 Mod Manager (teutinsa) and its descendants.
// ---------------------------------------------------------------------------

/// Per-mod state keyed by manifest GUID, with the profile's name if known.
pub type GuidHints = (Option<String>, HashMap<Uuid, ProfileHint>);

/// Profile state next to a folder of mods (in it or its parent):
/// - `profiles.json` (the 2.0 rewrite, and DDMM itself): profiles with
///   `Configs` in load order; the active one is used.
/// - `enabled.json`, 1.x: `[{Guid, Enabled, Toggled, Selected}]` in load
///   order.
/// - `enabled.json`, the original 2024 manager: `{"<guid>": optionIndex}`
///   for the enabled mods only.
pub fn load_guid_hints(root: &Path) -> Option<GuidHints> {
    for dir in [Some(root), root.parent()].into_iter().flatten() {
        if let Ok(data) = std::fs::read(dir.join("profiles.json")) {
            match serde_json::from_slice::<ProfilesConfig>(&data) {
                Ok(config) => {
                    let profile = config.profiles.get(config.active as usize).or_else(|| config.profiles.first())?;
                    let hints = profile
                        .configs()
                        .iter()
                        .enumerate()
                        .map(|(order, c)| (*c.uuid(), hint_from_config(c, order)))
                        .collect();
                    return Some((Some(profile.name().to_string()), hints));
                }
                Err(e) => log::info!("Import: {:?} isn't a profile file DDMM can read ({e}); ignoring it.", dir.join("profiles.json")),
            }
        }
        if let Ok(data) = std::fs::read(dir.join("enabled.json")) {
            if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                let mut hints = HashMap::new();
                match value {
                    Value::Array(entries) => {
                        for (order, e) in entries.iter().enumerate() {
                            let Some(guid) = e.get("Guid").and_then(Value::as_str).and_then(|g| Uuid::parse_str(g).ok()) else { continue };
                            let list = |k: &str| e.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
                            hints.insert(
                                guid,
                                ProfileHint {
                                    enabled: e.get("Enabled").and_then(Value::as_bool).unwrap_or(true),
                                    order,
                                    toggled: Some(list("Toggled").iter().filter_map(Value::as_bool).collect()),
                                    selected: Some(list("Selected").iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect()),
                                },
                            );
                        }
                    }
                    Value::Object(map) => {
                        for (order, (guid, index)) in map.iter().enumerate() {
                            let Ok(guid) = Uuid::parse_str(guid) else { continue };
                            let selected = index.as_u64().map(|n| vec![n as usize]);
                            hints.insert(guid, ProfileHint { enabled: true, order, toggled: None, selected });
                        }
                    }
                    _ => continue,
                }
                return Some((None, hints));
            }
        }
    }
    None
}

fn hint_from_config(c: &Config, order: usize) -> ProfileHint {
    match c {
        Config::Legacy { enabled, selected, .. } => {
            ProfileHint { enabled: *enabled, order, toggled: None, selected: Some(vec![*selected]) }
        }
        Config::V1 { enabled, toggled, selected, .. } | Config::V2 { enabled, toggled, selected, .. } => ProfileHint {
            enabled: *enabled,
            order,
            toggled: Some(toggled.clone()),
            selected: Some(selected.clone()),
        },
    }
}

// ---------------------------------------------------------------------------
// Vortex (with the Helldivers 2 extension): the extension keeps each
// profile's patch order in `<profileId>_patch_order.json` next to the
// staging folder (`%APPDATA%\Vortex\helldivers2\`), keyed by the staged
// mod's folder name.
// ---------------------------------------------------------------------------

/// Per-mod state keyed by the mod's staging folder name.
pub fn load_patch_order(staging: &Path) -> Option<HashMap<String, ProfileHint>> {
    let dir = staging.parent()?;
    let newest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with("_patch_order.json"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    let value: Value = serde_json::from_slice(&std::fs::read(newest.path()).ok()?).ok()?;
    let mut out = HashMap::new();
    for (order, e) in value.as_array()?.iter().enumerate() {
        let Some(folder) = str_field(e, "modId").or_else(|| str_field(e, "id")) else { continue };
        out.insert(
            folder,
            ProfileHint {
                enabled: e.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                order,
                toggled: None,
                selected: None,
            },
        );
    }
    Some(out)
}
