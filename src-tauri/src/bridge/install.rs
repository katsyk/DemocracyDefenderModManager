//! The backend half of installing a mod handed off by the browser
//! extension: validate the finished download, figure out where it came
//! from, and install it -- reusing exactly the same archive-install and
//! in-place-update paths every other install route uses. Adding the mod to
//! a profile and/or deploying it (the `afterInstall` step) happens on the
//! frontend, which already owns that logic; see `bridge::server`.

use std::path::Path;

use crate::{
    commands::mods::{install_from_archive, install_update_from_archive},
    download,
    models::{manifest::Source, Mod},
    sources, AppState,
};

use super::protocol::ErrorCode;

#[derive(Debug)]
pub struct InstallOutcome {
    pub r#mod: Mod,
    pub updated: bool,
    pub warning: Option<String>,
}

/// Everything that can go wrong before/while installing the archive
/// itself, mapped to the protocol's own error codes.
#[derive(Debug)]
pub struct InstallError {
    pub code: ErrorCode,
    pub message: String,
}

impl InstallError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// Resolve the `Source` an install request's `pageUrl`/`downloadUrl`
/// describes: a recognized mod-page URL wins (structured provider + id);
/// otherwise fall back to a bare `url` source keyed off whichever URL is
/// available, with the provider guessed from its host.
pub fn resolve_source(page_url: Option<&str>, download_url: Option<&str>, page_version: Option<&str>) -> Option<Source> {
    if let Some(page_url) = page_url {
        if let Some(mut source) = sources::source_from_page_url(page_url) {
            source.version = page_version.map(str::to_string);
            return Some(source);
        }
    }

    let url = page_url.or(download_url)?;
    Some(Source {
        provider: sources::provider_from_url(url),
        id: None,
        url: Some(url.to_string()),
        version: page_version.map(str::to_string),
    })
}

/// Find an already-installed mod whose recorded sources include the same
/// `(provider, id)` pair as `source` -- the same "same source -> update in
/// place" rule "Update from {site}" already uses. Only meaningful when
/// `source.id` is `Some`; a bare `url` source (no id) never matches an
/// existing mod this way.
fn find_existing_by_source(mods: &[Mod], source: &Source) -> Option<uuid::Uuid> {
    let id = source.id.as_ref()?;
    mods.iter()
        .find(|m| {
            m.sources.iter().any(|s| {
                s.provider.eq_ignore_ascii_case(&source.provider) && s.page_url.is_some() && {
                    // ResolvedSource doesn't carry the raw id (see
                    // commands::updates::id_from_page_url for the same
                    // reasoning) -- recover it from the page URL the same
                    // way.
                    s.page_url
                        .as_deref()
                        .and_then(sources::source_from_page_url)
                        .and_then(|resolved| resolved.id)
                        .as_deref()
                        == Some(id.as_str())
                }
            })
        })
        .map(|m| m.guid())
}

/// Validate `file` per the protocol spec (a regular, non-symlink file,
/// ≤ 2 GiB, and a zip/7z/rar by magic bytes regardless of its extension --
/// the browser's own filename for a download is not to be trusted), then
/// install it: an in-place update if an installed mod shares this
/// request's source, otherwise a fresh install.
pub async fn install_file(
    state: &AppState,
    mods: &mut Vec<Mod>,
    file: &Path,
    page_url: Option<&str>,
    download_url: Option<&str>,
    page_version: Option<&str>,
) -> Result<InstallOutcome, InstallError> {
    let meta = tokio::fs::symlink_metadata(file)
        .await
        .map_err(|_| InstallError::new(ErrorCode::FileNotFound, "file does not exist"))?;
    if meta.file_type().is_symlink() {
        return Err(InstallError::new(ErrorCode::FileNotFound, "refusing to install a symlink"));
    }
    if !meta.is_file() {
        return Err(InstallError::new(ErrorCode::FileNotFound, "not a regular file"));
    }
    if meta.len() > download::MAX_DOWNLOAD_SIZE {
        return Err(InstallError::new(
            ErrorCode::NotArchive,
            format!("file exceeds the {} byte limit", download::MAX_DOWNLOAD_SIZE),
        ));
    }

    let mut header = [0u8; 8];
    {
        use tokio::io::AsyncReadExt;
        let mut f = tokio::fs::File::open(file)
            .await
            .map_err(|_| InstallError::new(ErrorCode::FileNotFound, "could not open file"))?;
        let read = f
            .read(&mut header)
            .await
            .map_err(|_| InstallError::new(ErrorCode::FileNotFound, "could not read file"))?;
        if download::sniff_archive_extension(&header[..read]).is_none() {
            return Err(InstallError::new(ErrorCode::NotArchive, "not a zip/7z/rar archive"));
        }
    }

    let source = resolve_source(page_url, download_url, page_version);

    let existing_guid = source.as_ref().and_then(|s| find_existing_by_source(mods, s));

    let (r#mod, warning) = match existing_guid {
        Some(guid) => install_update_from_archive(state, mods, file, guid)
            .await
            .map_err(|e| InstallError::new(ErrorCode::Internal, e.to_string()))?,
        None => install_from_archive(state, mods, file)
            .await
            .map_err(|e| InstallError::new(ErrorCode::Internal, e.to_string()))?,
    };

    let mut r#mod = r#mod;
    if let Some(source) = source {
        if let Err(e) = sources::write_origin_sidecar(&r#mod.directory, vec![source]).await {
            log::error!("Failed to write bridge-install origin sidecar: {}", e);
        }
        r#mod.resolve_sources().await;
        if let Some(existing) = mods.iter_mut().find(|m| m.guid() == r#mod.guid()) {
            *existing = r#mod.clone();
        }
    }

    Ok(InstallOutcome {
        r#mod,
        updated: existing_guid.is_some(),
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::manifest::Source as ManifestSource;

    #[test]
    fn resolve_source_prefers_page_url() {
        let source = resolve_source(
            Some("https://ayakamods.com/mods/hd2-auto-reload.4084/"),
            Some("https://ayakamods.com/mods/hd2-auto-reload.4084/download"),
            Some("2026-09-24"),
        )
        .unwrap();
        assert_eq!(source.provider, "ayakamods");
        assert_eq!(source.id.as_deref(), Some("4084"));
        assert_eq!(source.version.as_deref(), Some("2026-09-24"));
    }

    #[test]
    fn resolve_source_falls_back_to_download_url_host() {
        let source = resolve_source(None, Some("https://cdn.example.com/mods/cool.zip"), None).unwrap();
        assert_eq!(source.provider, "url");
        assert_eq!(source.url.as_deref(), Some("https://cdn.example.com/mods/cool.zip"));
        assert!(source.id.is_none());
    }

    #[test]
    fn resolve_source_none_when_no_urls() {
        assert!(resolve_source(None, None, None).is_none());
    }

    #[test]
    fn resolve_source_unrecognized_page_url_falls_back_to_url_provider() {
        let source = resolve_source(Some("https://example.com/mods/cool"), None, None).unwrap();
        assert_eq!(source.provider, "url");
        assert_eq!(source.url.as_deref(), Some("https://example.com/mods/cool"));
    }

    fn dummy_mod(guid: uuid::Uuid, sources: Vec<crate::sources::ResolvedSource>) -> Mod {
        Mod {
            manifest: crate::models::manifest::Manifest::Legacy(crate::models::manifest::legacy::Manifest {
                guid,
                name: "Existing".to_string(),
                description: String::new(),
                icon_path: None,
                options: None,
            }),
            directory: std::path::PathBuf::from("/nonexistent"),
            sources,
        }
    }

    #[test]
    fn find_existing_by_source_matches_same_provider_and_id() {
        let guid = uuid::Uuid::new_v4();
        let resolved = crate::sources::resolve(&ManifestSource {
            provider: "ayakamods".to_string(),
            id: Some("4084".to_string()),
            url: None,
            version: None,
        });
        let mods = vec![dummy_mod(guid, vec![resolved])];

        let source = ManifestSource {
            provider: "ayakamods".to_string(),
            id: Some("4084".to_string()),
            url: None,
            version: None,
        };
        assert_eq!(find_existing_by_source(&mods, &source), Some(guid));
    }

    #[test]
    fn find_existing_by_source_no_match_for_different_id() {
        let guid = uuid::Uuid::new_v4();
        let resolved = crate::sources::resolve(&ManifestSource {
            provider: "ayakamods".to_string(),
            id: Some("4084".to_string()),
            url: None,
            version: None,
        });
        let mods = vec![dummy_mod(guid, vec![resolved])];

        let source = ManifestSource {
            provider: "ayakamods".to_string(),
            id: Some("9999".to_string()),
            url: None,
            version: None,
        };
        assert_eq!(find_existing_by_source(&mods, &source), None);
    }

    #[test]
    fn find_existing_by_source_none_without_id() {
        let source = ManifestSource {
            provider: "url".to_string(),
            id: None,
            url: Some("https://example.com/x.zip".to_string()),
            version: None,
        };
        assert_eq!(find_existing_by_source(&[], &source), None);
    }

    #[tokio::test]
    async fn install_file_rejects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();
        let err = install_file(&state, &mut mods, &dir.path().join("nope.zip"), None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::FileNotFound);
    }

    #[tokio::test]
    async fn install_file_rejects_non_archive_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("innocuous.zip"); // extension lies
        tokio::fs::write(&file, b"not an archive at all").await.unwrap();

        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();
        let err = install_file(&state, &mut mods, &file, None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotArchive);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn install_file_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.zip");
        tokio::fs::write(&real, b"PK\x03\x04rest").await.unwrap();
        let link = dir.path().join("link.zip");
        tokio::fs::symlink(&real, &link).await.unwrap();

        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();
        let err = install_file(&state, &mut mods, &link, None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::FileNotFound);
    }
}
