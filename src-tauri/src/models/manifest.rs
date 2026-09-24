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
        pub description: String,
        pub include: Optional<Vec<PathBuf>>,
        pub image: Optional<PathBuf>,
        pub sub_options: Optional<Vec<SubOption>>,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct SubOption {
        pub name: String,
        pub description: String,
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
        pub description: String,
        pub include: Vec<PathBuf>,
        pub image: Optional<PathBuf>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Category {
        pub guid: Uuid,
        pub name: String,
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