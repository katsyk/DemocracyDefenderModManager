//! Wire types for the browser bridge protocol (v1) -- see
//! `docs/development/bridge-protocol.md`, the contract this module and the
//! extension are both implemented against.
//!
//! Deliberately `camelCase` on the wire (unlike the rest of this app's
//! Tauri commands, which are `PascalCase`): this is a protocol shared with
//! a browser extension, not an internal Tauri IPC call.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum size, in bytes, of a single native-messaging frame in either
/// direction. The browser itself caps host->browser messages at 1 MB and
/// browser->host at 64 MB; we hold everything to 1 MB regardless.
pub const MAX_MESSAGE_BYTES: u32 = 1024 * 1024;

/// How many installs may be queued (serialized, one at a time) before a new
/// request is rejected with `BUSY` instead of waiting.
pub const MAX_QUEUE_DEPTH: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    AppNotRunning,
    BadRequest,
    Unsupported,
    ForbiddenOrigin,
    Declined,
    NotArchive,
    UnsafeArchive,
    FileNotFound,
    GameNotFound,
    DeployFailed,
    Busy,
    Internal,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorReply {
    pub id: String,
    pub ok: bool,
    pub error: ErrorDetail,
}

impl ErrorReply {
    pub fn new(id: impl Into<String>, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ok: false,
            error: ErrorDetail {
                code,
                message: message.into(),
            },
        }
    }

    pub fn to_line(&self) -> String {
        // Infallible: every field is a plain String/bool/enum.
        serde_json::to_string(self).unwrap()
    }
}

/// A request after its envelope (`id`, `type`) has been read but before
/// it's been matched against a specific message shape.
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloRequest {
    pub id: String,
    #[serde(default)]
    pub extension_version: Option<String>,
    #[serde(default)]
    pub browser: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    pub id: String,
    pub ok: bool,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub app_version: String,
    pub protocol: u32,
    pub after_install: String,
    pub game_found: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequest {
    pub id: String,
    pub file: String,
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub page_version: Option<String>,
    #[serde(default)]
    pub after_install: Option<String>,
    /// Added by the host relay itself (see `host::relay`), never sent by
    /// the extension directly -- the app trusts this field only because
    /// the host, not the browser, sets it.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledModSummary {
    pub guid: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<InstalledModSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledModSource {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledReply {
    pub id: String,
    pub ok: bool,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "mod")]
    pub installed_mod: InstalledModSummary,
    pub updated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_to_profile: Option<String>,
    pub deployed: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub id: String,
    pub page_url: String,
    #[serde(default)]
    pub page_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryModSummary {
    pub guid: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryReply {
    pub id: String,
    pub ok: bool,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#mod: Option<QueryModSummary>,
    /// `None` serializes as JSON `null` -- "never guessed", per spec.
    pub update_available: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReply {
    pub id: String,
    pub ok: bool,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub game_found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    pub mod_count: usize,
    pub busy: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_serializes_screaming_snake_case() {
        let json = serde_json::to_string(&ErrorCode::AppNotRunning).unwrap();
        assert_eq!(json, "\"APP_NOT_RUNNING\"");
        let json = serde_json::to_string(&ErrorCode::UnsafeArchive).unwrap();
        assert_eq!(json, "\"UNSAFE_ARCHIVE\"");
    }

    #[test]
    fn envelope_parses_id_and_type_ignoring_extra_fields() {
        let raw = r#"{"id":"1","type":"hello","extensionVersion":"1.0.0","browser":"chrome","futureField":42}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.id, "1");
        assert_eq!(env.kind, "hello");
    }

    #[test]
    fn envelope_missing_type_fails_to_parse() {
        let raw = r#"{"id":"1"}"#;
        let result: Result<Envelope, _> = serde_json::from_str(raw);
        assert!(result.is_err());
    }

    #[test]
    fn install_request_parses_full_shape() {
        let raw = r#"{
            "id": "2", "type": "install",
            "file": "C:\\Users\\me\\Downloads\\Cool Mod-4084-1-0.zip",
            "pageUrl": "https://ayakamods.com/mods/cool-mod.4084/",
            "downloadUrl": "https://ayakamods.com/mods/cool-mod.4084/download",
            "pageVersion": "1.0",
            "afterInstall": null
        }"#;
        let req: InstallRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.id, "2");
        assert_eq!(req.file, "C:\\Users\\me\\Downloads\\Cool Mod-4084-1-0.zip");
        assert_eq!(req.page_url.as_deref(), Some("https://ayakamods.com/mods/cool-mod.4084/"));
        assert!(req.after_install.is_none());
        assert!(req.origin.is_none());
    }

    #[test]
    fn install_request_missing_file_fails_to_parse() {
        let raw = r#"{"id":"2","type":"install","pageUrl":"https://example.com"}"#;
        let result: Result<InstallRequest, _> = serde_json::from_str(raw);
        assert!(result.is_err());
    }

    #[test]
    fn hello_reply_serializes_camel_case() {
        let reply = HelloReply {
            id: "1".to_string(),
            ok: true,
            kind: "hello",
            app_version: "2.0.0-rc.4".to_string(),
            protocol: PROTOCOL_VERSION,
            after_install: "deploy".to_string(),
            game_found: true,
        };
        let value = serde_json::to_value(&reply).unwrap();
        assert_eq!(value["appVersion"], "2.0.0-rc.4");
        assert_eq!(value["afterInstall"], "deploy");
        assert_eq!(value["gameFound"], true);
    }

    #[test]
    fn query_reply_update_available_null_when_none() {
        let reply = QueryReply {
            id: "3".to_string(),
            ok: true,
            kind: "queryResult",
            installed: false,
            r#mod: None,
            update_available: None,
        };
        let value = serde_json::to_value(&reply).unwrap();
        assert!(value["updateAvailable"].is_null());
    }

    #[test]
    fn query_reply_mod_uses_camel_case_installed_version() {
        let reply = QueryReply {
            id: "3".to_string(),
            ok: true,
            kind: "queryResult",
            installed: true,
            r#mod: Some(QueryModSummary {
                guid: "g".to_string(),
                name: "Cool Mod".to_string(),
                installed_version: Some("1.0".to_string()),
            }),
            update_available: Some(true),
        };
        let value = serde_json::to_value(&reply).unwrap();
        assert_eq!(value["mod"]["installedVersion"], "1.0");
        assert!(value["mod"].get("installed_version").is_none());
    }

    #[test]
    fn error_reply_round_trips_expected_shape() {
        let reply = ErrorReply::new("2", ErrorCode::Declined, "You chose not to install this mod.");
        let value: serde_json::Value = serde_json::from_str(&reply.to_line()).unwrap();
        assert_eq!(value["id"], "2");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "DECLINED");
    }
}
