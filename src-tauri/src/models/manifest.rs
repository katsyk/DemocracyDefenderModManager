use serde::{Deserialize, Serialize};

/// A single, provider-neutral declaration of where a mod can be found.
///
/// `provider` is free-form and lowercase-normalized; a handful of values are
/// well-known ("nexus", "modworkshop", "github", "gamebanana", "url") but any
/// other string is accepted so new sites never require a schema change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Source {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

pub mod legacy {
    use std::path::PathBuf;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Manifest {
        pub guid: Uuid,
        pub name: String,
        #[serde(default)]
        pub description: String,
        pub icon_path: Option<PathBuf>,
        pub options: Option<Vec<String>>,
    }
}

pub mod v1 {
    use std::path::PathBuf;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;
    use crate::models::Version;
    use super::Source;

    type Optional<T> = std::option::Option<T>;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Manifest {
        pub version: Version<1>,
        pub guid: Uuid,
        pub name: String,
        #[serde(default)]
        pub description: String,
        pub icon_path: Optional<PathBuf>,
        pub options: Optional<Vec<Option>>,
        #[serde(default, skip_serializing_if = "std::option::Option::is_none")]
        pub nexus_data: Optional<NexusData>,
        #[serde(default, skip_serializing_if = "std::option::Option::is_none")]
        pub sources: Optional<Vec<Source>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Option {
        pub name: String,
        #[serde(default)]
        pub description: String,
        pub include: Optional<Vec<PathBuf>>,
        pub image: Optional<PathBuf>,
        pub sub_options: Optional<Vec<SubOption>>,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct SubOption {
        pub name: String,
        #[serde(default)]
        pub description: String,
        /// A sub-option with no files (e.g. "None") may omit this.
        #[serde(default)]
        pub include: Vec<PathBuf>,
        pub image: Optional<PathBuf>,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct NexusData {
        pub mod_id: u64,
        pub version: String,
    }
}

pub mod v2 {
    use std::path::PathBuf;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;
    use crate::models::Version;
    use super::Source;

    type Optional<T> = std::option::Option<T>;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Manifest {
        pub version: Version<2>,
        pub guid: Uuid,
        pub name: String,
        #[serde(default)]
        pub description: String,
        pub icon_path: Optional<PathBuf>,
        pub options: Optional<Vec<Option>>,
        pub categories: Optional<Vec<Category>>,
        pub tags: Optional<Vec<String>>,
        #[serde(default, skip_serializing_if = "std::option::Option::is_none")]
        pub nexus_data: Optional<NexusData>,
        #[serde(default, skip_serializing_if = "std::option::Option::is_none")]
        pub sources: Optional<Vec<Source>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Option {
        pub guid: Uuid,
        pub name: String,
        pub category_ref: Optional<Uuid>,
        #[serde(default)]
        pub description: String,
        pub include: Optional<Vec<PathBuf>>,
        pub image: Optional<PathBuf>,
        pub sub_options: Optional<Vec<SubOption>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct SubOption {
        pub guid: Uuid,
        pub name: String,
        #[serde(default)]
        pub description: String,
        /// A sub-option with no files (e.g. "None") may omit this.
        #[serde(default)]
        pub include: Vec<PathBuf>,
        pub image: Optional<PathBuf>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Category {
        pub guid: Uuid,
        pub name: String,
        #[serde(default)]
        pub description: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct NexusData {
        pub mod_id: u64
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Manifest {
    Legacy(legacy::Manifest),
    V1(v1::Manifest),
    V2(v2::Manifest),
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        let raw = serde_json::Value::deserialize(deserializer)?;

        if let Some(version) = raw.get("Version").and_then(|v| v.as_u64()) {
            match version {
                1 => {
                    v1::Manifest::deserialize(raw)
                        .map(Manifest::V1)
                        .map_err(|e| Error::custom(format!("v1 manifest deserialization failed: {}", e)))
                }
                2 => {
                    v2::Manifest::deserialize(raw)
                        .map(Manifest::V2)
                        .map_err(|e| Error::custom(format!("v2 manifest deserialization failed: {}", e)))
                }
                v => Err(Error::custom(format!("unknown manifest version {}", v)))
            }
        } else {
            legacy::Manifest::deserialize(raw)
                .map(Manifest::Legacy)
                .map_err(|e| Error::custom(format!("legacy manifest deserialization failed: {}", e)))
        }
    }
}

/// Every key the manifest formats use, in their canonical spelling. Used to
/// fix up authors' key-case variations (`GUID`, `iconPath`, `icon_path`).
const KNOWN_KEYS: &[&str] = &[
    "Version", "Guid", "Name", "Description", "IconPath", "Options", "NexusData", "Sources",
    "Categories", "Tags", "Include", "Image", "SubOptions", "CategoryRef", "ModId", "Provider",
    "Id", "Url",
];

impl Manifest {
    /// Parse a `manifest.json`'s raw bytes, tolerating what real-world mod
    /// authors (mostly on Windows, mostly editing by hand) actually ship:
    ///
    /// - a UTF-8 byte-order mark, UTF-16 (LE/BE, with or without BOM), or
    ///   legacy 8-bit text;
    /// - `//` and `/* */` comments and trailing commas;
    /// - key case/style variations (`GUID`, `guid`, `iconPath`, `icon_path`);
    /// - `"Version": "1"` or `1.0` instead of `1`;
    /// - a missing `Description`.
    ///
    /// Anything still unreadable is an error that starts with `origin` (e.g.
    /// the file name) and, for JSON syntax errors, says the line and column.
    pub fn parse(data: &[u8], origin: &str) -> anyhow::Result<Manifest> {
        let text = decode_manifest_text(data, origin);

        let mut value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(strict) => match serde_json::from_str(&strip_comments_and_trailing_commas(&text)) {
                Ok(v) => {
                    log::warn!(
                        "{}: not strict JSON ({}); accepted after ignoring comments/trailing commas",
                        origin, strict
                    );
                    v
                }
                Err(_) => anyhow::bail!("{} is not valid JSON: {}", origin, strict),
            },
        };

        if !value.is_object() {
            anyhow::bail!("{} is not a manifest: expected a JSON object at the top level", origin);
        }
        normalize_keys(&mut value);
        normalize_version(&mut value);

        serde_json::from_value(value).map_err(|e| anyhow::anyhow!("{}: {}", origin, e))
    }
}

fn decode_manifest_text(data: &[u8], origin: &str) -> String {
    fn utf16(data: &[u8], le: bool) -> Option<String> {
        if data.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16(&units).ok()
    }

    if let Some(rest) = data.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        if let Some(s) = utf16(rest, true) {
            return s;
        }
    }
    if let Some(rest) = data.strip_prefix(&[0xFE, 0xFF]) {
        if let Some(s) = utf16(rest, false) {
            return s;
        }
    }
    // UTF-16 without a BOM: JSON starts with an ASCII character, so one of
    // the first two bytes is zero.
    if data.len() >= 2 && data[0] != 0 && data[1] == 0 {
        if let Some(s) = utf16(data, true) {
            return s;
        }
    }
    if data.len() >= 2 && data[0] == 0 && data[1] != 0 {
        if let Some(s) = utf16(data, false) {
            return s;
        }
    }

    match std::str::from_utf8(data) {
        Ok(s) => s.to_string(),
        Err(_) => {
            // Saved as a Windows "ANSI" code page: map bytes 1:1 (Latin-1),
            // which keeps all the ASCII structure intact.
            log::warn!("{}: not UTF-8; reading it as Latin-1", origin);
            data.iter().map(|&b| b as char).collect()
        }
    }
}

/// Remove `//` line comments, `/* */` block comments and trailing commas
/// before `}`/`]`, leaving string contents (e.g. `"https://..."`) alone.
fn strip_comments_and_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;

    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                // Drop the comma if the next non-whitespace, non-comment
                // character closes the object/array.
                let mut j = i + 1;
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'/') {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                        continue;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'*') {
                        j += 2;
                        while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                            j += 1;
                        }
                        j += 2;
                        continue;
                    }
                    break;
                }
                if !matches!(chars.get(j), Some('}') | Some(']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Rename keys that match a [`KNOWN_KEYS`] entry ignoring case and `_`/`-`
/// to that canonical spelling, at every depth. Never overwrites a key that
/// is already present in canonical form.
fn normalize_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let renames: Vec<(String, &'static str)> = map
                .keys()
                .filter_map(|k| {
                    let squashed: String = k.chars().filter(|c| *c != '_' && *c != '-').collect();
                    KNOWN_KEYS
                        .iter()
                        .find(|known| known.eq_ignore_ascii_case(&squashed) && *known != k)
                        .map(|known| (k.clone(), *known))
                })
                .collect();
            for (from, to) in renames {
                if !map.contains_key(to) {
                    if let Some(v) = map.remove(&from) {
                        map.insert(to.to_string(), v);
                    }
                }
            }
            for v in map.values_mut() {
                normalize_keys(v);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(normalize_keys),
        _ => {}
    }
}

/// `"Version": "1"` / `1.0` -> `1`, top level only (NexusData's own
/// `Version` is a mod version string and must stay one).
fn normalize_version(value: &mut serde_json::Value) {
    let Some(version) = value.get_mut("Version") else { return };
    let as_int = match &*version {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        serde_json::Value::Number(n) if !n.is_u64() => n.as_f64(),
        _ => None,
    };
    if let Some(f) = as_int {
        if f.fract() == 0.0 && f >= 0.0 {
            *version = serde_json::Value::from(f as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_manifest_with_nexus_data_only_loads_and_round_trips() {
        let json = serde_json::json!({
            "Version": 1,
            "Guid": "00000000-0000-0000-0000-000000000001",
            "Name": "Test Mod",
            "Description": "A test mod",
            "NexusData": {
                "ModId": 123,
                "Version": "1.0.0"
            }
        });

        let manifest: Manifest = serde_json::from_value(json).expect("should deserialize");
        let v1 = match &manifest {
            Manifest::V1(m) => m,
            _ => panic!("expected v1 manifest"),
        };

        assert!(v1.sources.is_none());
        assert!(v1.nexus_data.is_some());

        let nexus_id = v1.nexus_data.as_ref().unwrap().mod_id;
        let legacy_source = crate::sources::merge_sources(
            v1.sources.as_deref(),
            Some(Source {
                provider: "nexus".to_string(),
                id: Some(nexus_id.to_string()),
                url: None,
                version: Some(v1.nexus_data.as_ref().unwrap().version.clone()),
            }),
            &[],
        );
        assert_eq!(legacy_source.len(), 1);
        assert_eq!(
            legacy_source[0].page_url.as_deref(),
            Some("https://www.nexusmods.com/helldivers2/mods/123")
        );

        // Re-serializing must keep NexusData and must NOT introduce a Sources key.
        let value = serde_json::to_value(&manifest).unwrap();
        assert!(value.get("NexusData").is_some());
        assert!(value.get("Sources").is_none());
    }

    #[test]
    fn v2_manifest_with_sources_and_matching_nexus_data_has_no_duplicate() {
        let json = serde_json::json!({
            "Version": 2,
            "Guid": "00000000-0000-0000-0000-000000000002",
            "Name": "Test Mod",
            "Description": "A test mod",
            "Sources": [
                { "Provider": "nexus", "Id": "123" }
            ],
            "NexusData": {
                "ModId": 123
            }
        });

        let manifest: Manifest = serde_json::from_value(json).expect("should deserialize");
        let v2 = match &manifest {
            Manifest::V2(m) => m,
            _ => panic!("expected v2 manifest"),
        };

        let nexus_id = v2.nexus_data.as_ref().unwrap().mod_id;
        let resolved = crate::sources::merge_sources(
            v2.sources.as_deref(),
            Some(Source {
                provider: "nexus".to_string(),
                id: Some(nexus_id.to_string()),
                url: None,
                version: None,
            }),
            &[],
        );

        assert_eq!(resolved.len(), 1, "duplicate nexus source was not deduplicated");
    }

    fn fixture(name: &str) -> anyhow::Result<Manifest> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/manifests")
            .join(name);
        Manifest::parse(&std::fs::read(&path).unwrap(), name)
    }

    fn expect_v1(name: &str) -> v1::Manifest {
        match fixture(name) {
            Ok(Manifest::V1(m)) => m,
            other => panic!("{name}: expected v1, got {other:?}"),
        }
    }

    #[test]
    fn parses_utf8_bom() {
        let m = expect_v1("utf8_bom.json");
        assert_eq!(m.description, "Café – fixture");
    }

    #[test]
    fn parses_utf16_with_and_without_bom() {
        assert_eq!(expect_v1("utf16le_bom.json").description, "Café – fixture");
        assert_eq!(expect_v1("utf16be_no_bom.json").description, "Café – fixture");
    }

    #[test]
    fn parses_comments_and_trailing_commas_but_not_inside_strings() {
        let m = expect_v1("comments_trailing_commas.json");
        assert_eq!(m.description, "See https://example.com/mod // not a comment");
        assert_eq!(m.options.unwrap()[0].include.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn parses_key_case_variants_and_braced_guid() {
        let m = expect_v1("key_case_variants.json");
        assert_eq!(m.guid.to_string(), "5b1c7a3e-6f0b-4b43-9a55-0d7f1c2e8a11");
        assert_eq!(m.icon_path.as_deref(), Some(std::path::Path::new("icon.png")));
        let opts = m.options.unwrap();
        assert_eq!(opts[0].name, "Red");
        assert_eq!(opts[0].sub_options.as_ref().unwrap()[0].include.len(), 1);
    }

    #[test]
    fn parses_string_version_and_keeps_nexus_version_a_string() {
        let m = expect_v1("version_as_string.json");
        assert_eq!(m.nexus_data.unwrap().version, "1.2");
    }

    #[test]
    fn parses_float_version_and_missing_description() {
        match fixture("version_as_float_no_description.json") {
            Ok(Manifest::V2(m)) => assert_eq!(m.description, ""),
            other => panic!("expected v2, got {other:?}"),
        }
    }

    #[test]
    fn parses_backslash_paths_verbatim() {
        // Kept as written; resolved against the files on disk later by
        // `utils::fix_path_casing` / deploy.
        let m = expect_v1("backslash_paths.json");
        assert_eq!(m.icon_path.unwrap().to_string_lossy(), "Images\\icon.png");
    }

    #[test]
    fn syntax_errors_name_the_file_line_and_column() {
        let err = fixture("broken_syntax.json").unwrap_err().to_string();
        assert!(err.contains("broken_syntax.json"), "{err}");
        assert!(err.contains("line 4"), "{err}");
    }

    #[test]
    fn schema_errors_name_the_file_and_field() {
        let err = Manifest::parse(br#"{"Version": 1, "Guid": "not-a-guid", "Name": "x"}"#, "m.json")
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("m.json"), "{err}");
        assert!(err.to_lowercase().contains("invalid"), "{err}");
    }

    #[test]
    fn sub_option_without_include_is_accepted() {
        let m = Manifest::parse(br#"{"Version": 1, "Guid": "5b1c7a3e-6f0b-4b43-9a55-0d7f1c2e8a11", "Name": "x",
            "Options": [{"Name": "Hip", "Description": "", "SubOptions": [{"Name": "None", "Description": ""}]}]}"#, "m.json")
            .unwrap();
        assert!(matches!(m, Manifest::V1(_)));
    }

    #[test]
    fn legacy_manifest_without_version_still_loads() {
        let json = serde_json::json!({
            "Guid": "00000000-0000-0000-0000-000000000003",
            "Name": "Old Mod",
            "Description": "",
        });

        let manifest: Manifest = serde_json::from_value(json).expect("should deserialize");
        assert!(matches!(manifest, Manifest::Legacy(_)));
    }
}