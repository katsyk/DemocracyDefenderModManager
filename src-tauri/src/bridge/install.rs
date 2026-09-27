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
    /// The source recorded for this install (provider + id when `pageUrl`
    /// was a recognized mod page), echoed back in the `installed` reply.
    pub source: Option<Source>,
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

    /// Map a failure from the shared archive-install path to a protocol
    /// error code. The archive module rejects path traversal, absolute
    /// paths and symlink entries with an "unsafe path" error; the protocol
    /// reports those as `UNSAFE_ARCHIVE` rather than a generic `INTERNAL`.
    fn from_install_failure(message: String) -> Self {
        let code = if message.contains("unsafe path") {
            ErrorCode::UnsafeArchive
        } else {
            ErrorCode::Internal
        };
        Self { code, message }
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

/// Whether `resolve_source` took its (provider, id) from a recognized
/// mod-page URL -- the only case where an in-place update is allowed.
fn source_is_from_mod_page(page_url: Option<&str>) -> bool {
    page_url
        .and_then(sources::source_from_page_url)
        .is_some_and(|s| s.id.is_some())
}

/// Every installed mod whose recorded sources include the same
/// `(provider, id)` pair as `source`. Only meaningful when `source.id` is
/// `Some`; a bare `url` source (no id) never matches an existing mod.
fn mods_with_source<'a>(mods: &'a [Mod], source: &'a Source) -> impl Iterator<Item = &'a Mod> + 'a {
    let id = source.id.clone();
    mods.iter().filter(move |m| {
        let Some(id) = id.as_deref() else { return false };
        m.sources.iter().any(|s| {
            s.provider.eq_ignore_ascii_case(&source.provider)
                && s.page_url
                    .as_deref()
                    .and_then(sources::source_from_page_url)
                    .and_then(|resolved| resolved.id)
                    .as_deref()
                    == Some(id)
        })
    })
}

/// Whether `incoming` (the name of the file being installed) is another
/// version of `recorded` (the file an installed mod was installed from),
/// rather than a different file offered on the same mod page.
///
/// - Nexus Mods: both names are parsed (see `parse_nexus_archive_name`);
///   same when they're from the same mod and carry the same file name.
///   Versions and upload times differ between versions, so they don't
///   count.
/// - Everything else (and a Nexus name that doesn't parse): the names'
///   `file_shape` (letters only, GameBanana's upload suffix removed), so
///   `cool_mod_v1_ab12c.zip` and `cool_mod_v2_ff3bb.zip` are the same file
///   but `cool_mod_red_ab12c.zip` and `cool_mod_blue_ff3bb.zip` are not.
pub fn is_same_file(provider: &str, recorded: &str, incoming: &str) -> bool {
    if provider.eq_ignore_ascii_case("nexus") {
        if let (Some(a), Some(b)) =
            (sources::parse_nexus_archive_name(recorded), sources::parse_nexus_archive_name(incoming))
        {
            return a.mod_id == b.mod_id && a.name.trim().eq_ignore_ascii_case(b.name.trim());
        }
    }
    let (a, b) = (crate::providers::file_shape(recorded), crate::providers::file_shape(incoming));
    !a.is_empty() && a == b
}

/// Which installed mod an install of `incoming_name` from `source`'s page
/// updates in place, if any:
///
/// - the mod whose recorded file (see `sources::InstalledFile`) is this
///   same file ([`is_same_file`]);
/// - otherwise, when exactly one mod came from this page and DDMM never
///   recorded which file it was (installed before it did), that mod --
///   there is nothing to tell them apart by, and this is how updates of
///   such mods always worked;
/// - otherwise none: it's another file of that page, installed alongside.
async fn find_update_target(mods: &[Mod], source: &Source, incoming_name: &str) -> Option<uuid::Uuid> {
    let candidates: Vec<&Mod> = mods_with_source(mods, source).collect();
    let mut unrecorded = Vec::new();
    for m in &candidates {
        let recorded = sources::load_origin_sidecar(&m.directory).await.and_then(|s| {
            s.installed_files
                .into_iter()
                .find(|f| f.provider.eq_ignore_ascii_case(&source.provider))
                .and_then(|f| f.file_name)
        });
        match recorded {
            Some(name) if is_same_file(&source.provider, &name, incoming_name) => return Some(m.guid()),
            Some(_) => {}
            None => unrecorded.push(m.guid()),
        }
    }
    match (candidates.len(), unrecorded.as_slice()) {
        (1, [only]) => Some(*only),
        _ => None,
    }
}

/// Validate `file` per the protocol spec (a regular, non-symlink file,
/// ≤ 2 GiB, and a zip/7z/rar by the same detection as every other install
/// route: content first, never the browser's file name alone), then
/// install it: an in-place update if an installed mod came from this
/// request's source *and* is this same file (see [`find_update_target`]),
/// otherwise a fresh install.
pub async fn install_file(
    state: &AppState,
    mods: &mut Vec<Mod>,
    file: &Path,
    page_url: Option<&str>,
    download_url: Option<&str>,
    page_version: Option<&str>,
) -> Result<InstallOutcome, InstallError> {
    install_file_with(state, mods, file, page_url, download_url, page_version, Vec::new()).await
}

/// [`install_file`], also recording which exact file was installed (see
/// `sources::InstalledFile`).
pub async fn install_file_with(
    state: &AppState,
    mods: &mut Vec<Mod>,
    file: &Path,
    page_url: Option<&str>,
    download_url: Option<&str>,
    page_version: Option<&str>,
    installed_files: Vec<sources::InstalledFile>,
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

    // The same detection every other install route uses (Add, Add URL,
    // the handoff): content first, and a zip with something in front of it
    // is still a zip.
    let detect_path = file.to_path_buf();
    let detected = tokio::task::spawn_blocking(move || crate::archive::detect_format(&detect_path))
        .await
        .map_err(|e| InstallError::new(ErrorCode::Internal, format!("couldn't check the file: {e}")))?;
    if let Err(e) = detected {
        return Err(InstallError::new(
            ErrorCode::NotArchive,
            format!("the downloaded file can't be installed: {e:#}"),
        ));
    }

    let mut source = resolve_source(page_url, download_url, page_version);

    // Safety net: an in-place update replaces an installed mod's files, so
    // only do it when the (provider, id) was read off a mod-page URL. A
    // source derived any other way (downloadUrl host detection, an
    // unrecognized page) has no trustworthy id and always installs as a
    // new mod -- never over some other mod that happens to share a host.
    //
    // And only over the installed mod that *is* this file: one mod page can
    // offer several files that are installed side by side (GameBanana
    // variants, a Nexus main file and its optional files), and installing
    // one of them must never replace another.
    let incoming_name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    let existing_guid = match source.as_ref().filter(|_| source_is_from_mod_page(page_url)) {
        Some(s) => find_update_target(mods, s, &incoming_name).await,
        None => None,
    };

    // Remember which file this is (unless the caller already knows more),
    // so a later install from the same page can tell "a new version of
    // this file" from "another file of the same mod".
    let mut installed_files = installed_files;
    if let Some(src) = &source {
        if !incoming_name.is_empty() && !installed_files.iter().any(|f| f.provider.eq_ignore_ascii_case(&src.provider)) {
            installed_files.push(sources::InstalledFile {
                provider: src.provider.to_ascii_lowercase(),
                file_name: Some(incoming_name.clone()),
                ..Default::default()
            });
        }
    }

    // An update DDMM's own check found, arriving without a version (the
    // page didn't show one): record the version that check reported.
    if let (Some(guid), Some(src)) = (existing_guid, source.as_mut()) {
        if src.version.is_none() {
            src.version = crate::commands::updates::known_latest_version(state, guid, &src.provider).await;
        }
    }

    let (r#mod, warning) = match existing_guid {
        Some(guid) => install_update_from_archive(state, mods, file, guid)
            .await
            .map_err(|e| InstallError::from_install_failure(e.to_string()))?,
        None => install_from_archive(state, mods, file)
            .await
            .map_err(|e| InstallError::from_install_failure(e.to_string()))?,
    };

    let mut r#mod = r#mod;
    if let Some(source) = &source {
        if let Err(e) =
            sources::write_origin_sidecar_with_files(&r#mod.directory, vec![source.clone()], installed_files).await
        {
            log::error!("Failed to write bridge-install origin sidecar: {}", e);
        }
        r#mod.resolve_sources().await;
        if let Some(existing) = mods.iter_mut().find(|m| m.guid() == r#mod.guid()) {
            *existing = r#mod.clone();
        }
    }

    if let Some(guid) = existing_guid {
        crate::commands::updates::mark_mod_updated(
            state,
            guid,
            r#mod.guid(),
            source.as_ref().map(|s| s.provider.as_str()),
            source.as_ref().and_then(|s| s.version.as_deref()),
        )
        .await;
    }

    Ok(InstallOutcome {
        r#mod,
        updated: existing_guid.is_some(),
        warning,
        source,
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
    fn mods_with_source_matches_same_provider_and_id() {
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
        assert_eq!(mods_with_source(&mods, &source).next().map(|m| m.guid()), Some(guid));
    }

    #[test]
    fn mods_with_source_no_match_for_different_id() {
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
        assert_eq!(mods_with_source(&mods, &source).next().map(|m| m.guid()), None);
    }

    #[test]
    fn mods_with_source_none_without_id() {
        let source = ManifestSource {
            provider: "url".to_string(),
            id: None,
            url: Some("https://example.com/x.zip".to_string()),
            version: None,
        };
        assert_eq!(mods_with_source(&[], &source).next().map(|m| m.guid()), None);
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

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    const PAGE_URL: &str = "https://ayakamods.com/mods/test-mod.4084/";

    #[tokio::test]
    async fn install_file_reports_source_and_updates_in_place() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("mods")).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();

        let first = dir.path().join("Test Mod-4084-1-0.zip");
        make_zip(&first, &[("0123456789abcdef.patch_0", b"v1")]);
        let installed = install_file(&state, &mut mods, &first, Some(PAGE_URL), None, Some("1.0"))
            .await
            .unwrap();
        assert!(!installed.updated);
        let source = installed.source.as_ref().unwrap();
        assert_eq!(source.provider, "ayakamods");
        assert_eq!(source.id.as_deref(), Some("4084"));
        assert_eq!(source.version.as_deref(), Some("1.0"));
        assert_eq!(mods.len(), 1);

        let second = dir.path().join("Test Mod-4084-1-1.zip");
        make_zip(&second, &[("0123456789abcdef.patch_0", b"v2")]);
        let updated = install_file(&state, &mut mods, &second, Some(PAGE_URL), None, Some("1.1"))
            .await
            .unwrap();
        assert!(updated.updated);
        assert_eq!(updated.r#mod.guid(), installed.r#mod.guid());
        assert_eq!(updated.source.as_ref().unwrap().version.as_deref(), Some("1.1"));
        assert_eq!(mods.len(), 1);
    }

    fn patch_bytes(path: &Path) -> Vec<u8> {
        std::fs::read(path.join("0123456789abcdef.patch_0")).unwrap()
    }

    /// GameBanana variants: several files on one page, installed side by
    /// side. Installing one must never replace another; a new version of
    /// one replaces that one only.
    #[tokio::test]
    async fn variants_from_one_page_install_side_by_side() {
        const GB: &str = "https://gamebanana.com/mods/12345";
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("mods")).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();

        let red = dir.path().join("coolmod_red_ab12c.zip");
        make_zip(&red, &[("0123456789abcdef.patch_0", b"red 1")]);
        let red_out = install_file(&state, &mut mods, &red, Some(GB), None, None).await.unwrap();

        let blue = dir.path().join("coolmod_blue_ff3bb.zip");
        make_zip(&blue, &[("0123456789abcdef.patch_0", b"blue 1")]);
        let blue_out = install_file(&state, &mut mods, &blue, Some(GB), None, None).await.unwrap();
        assert!(!blue_out.updated, "another variant must install alongside, not over the first");
        assert_ne!(blue_out.r#mod.guid(), red_out.r#mod.guid());
        assert_eq!(mods.len(), 2);
        assert_eq!(patch_bytes(&red_out.r#mod.directory), b"red 1");

        // A new upload of the red variant updates red, and only red.
        let red2 = dir.path().join("coolmod_red_2_00aa1.zip");
        make_zip(&red2, &[("0123456789abcdef.patch_0", b"red 2")]);
        let red2_out = install_file(&state, &mut mods, &red2, Some(GB), None, None).await.unwrap();
        assert!(red2_out.updated);
        assert_eq!(red2_out.r#mod.guid(), red_out.r#mod.guid());
        assert_eq!(mods.len(), 2);
        assert_eq!(patch_bytes(&red2_out.r#mod.directory), b"red 2");
        assert_eq!(patch_bytes(&blue_out.r#mod.directory), b"blue 1");
    }

    /// A Nexus main file and an optional file of the same mod page, in both
    /// of Nexus's archive naming schemes.
    #[tokio::test]
    async fn nexus_main_and_optional_files_install_side_by_side() {
        const NEXUS: &str = "https://www.nexusmods.com/helldivers2/mods/1234";
        for (main1, optional, main2) in [
            (
                "Better Stims-1234-1-0-1718000000.zip",
                "Better Stims Optional-1234-1-0-1718000100.zip",
                "Better Stims-1234-1-1-1718100000.zip",
            ),
            (
                "Better Stims 1234 1.0 2026-06-24T03-45Z G8alq8bQH.zip",
                "Better Stims Optional 1234 1.0 2026-06-24T03-46Z aB3dE5fG7.zip",
                "Better Stims 1234 1.1 2026-07-01T10-00Z Zz9Yy8Xx7.zip",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::create_dir_all(dir.path().join("mods")).await.unwrap();
            let state = AppState::new(dir.path().to_path_buf());
            let mut mods = Vec::new();

            let f = dir.path().join(main1);
            make_zip(&f, &[("0123456789abcdef.patch_0", b"main 1")]);
            let main = install_file(&state, &mut mods, &f, Some(NEXUS), None, None).await.unwrap();

            let f = dir.path().join(optional);
            make_zip(&f, &[("0123456789abcdef.patch_0", b"optional")]);
            let opt = install_file(&state, &mut mods, &f, Some(NEXUS), None, None).await.unwrap();
            assert!(!opt.updated, "{optional}: an optional file must not replace the main file");
            assert_eq!(mods.len(), 2);
            assert_eq!(patch_bytes(&main.r#mod.directory), b"main 1");

            let f = dir.path().join(main2);
            make_zip(&f, &[("0123456789abcdef.patch_0", b"main 2")]);
            let updated = install_file(&state, &mut mods, &f, Some(NEXUS), None, None).await.unwrap();
            assert!(updated.updated, "{main2} is a new version of {main1}");
            assert_eq!(updated.r#mod.guid(), main.r#mod.guid());
            assert_eq!(patch_bytes(&opt.r#mod.directory), b"optional");
        }
    }

    /// A mod installed before DDMM recorded which file it was: the one mod
    /// from that page is still updated in place, as it always was.
    #[tokio::test]
    async fn a_mod_with_no_recorded_file_is_still_updated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mut mods, a_guid) = install_mod_a(dir.path()).await;
        let a_dir = mods[0].directory.clone();
        let mut sidecar = sources::load_origin_sidecar(&a_dir).await.unwrap();
        sidecar.installed_files.clear();
        sources::save_origin_sidecar(&a_dir, &sidecar).await.unwrap();

        let f = dir.path().join("Renamed Upload.zip");
        make_zip(&f, &[("0123456789abcdef.patch_0", b"A2")]);
        let out = install_file(&state, &mut mods, &f, Some(PAGE_URL), None, Some("2.0")).await.unwrap();
        assert!(out.updated);
        assert_eq!(out.r#mod.guid(), a_guid);
        assert_eq!(mods.len(), 1);
    }

    #[test]
    fn same_file_rules() {
        assert!(is_same_file("gamebanana", "cool_mod_v1_ab12c.zip", "cool_mod_v2_ff3bb.zip"));
        assert!(!is_same_file("gamebanana", "cool_mod_red_ab12c.zip", "cool_mod_blue_ff3bb.zip"));
        assert!(is_same_file("ayakamods", "Test Mod-4084-1-0.zip", "Test Mod-4084-1-1 (1).zip"));
        assert!(is_same_file("nexus", "Better Stims-1234-1-0-1718000000.zip", "Better Stims-1234-1-1-1718100000.zip"));
        assert!(!is_same_file("nexus", "Better Stims-1234-1-0-1718000000.zip", "Req-5678-1-0-1718000000.zip"));
        assert!(!is_same_file(
            "nexus",
            "Better Stims-1234-1-0-1718000000.zip",
            "Better Stims Optional-1234-1-0-1718000000.zip"
        ));
    }

    /// A zip with data in front of it (a self-extractor stub, or bytes an
    /// uploader prepended) installs through the extension exactly as it
    /// does through Add.
    #[tokio::test]
    async fn a_zip_with_prepended_data_installs_like_it_does_with_add() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("mods")).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();

        let mut data = b"#!/bin/sh\necho self-extractor stub\n".to_vec();
        data.extend(crate::archive::test_fixtures::zip(&[("0123456789abcdef.patch_0", b"data")]));
        let file = dir.path().join("prepended.zip");
        std::fs::write(&file, &data).unwrap();

        assert!(crate::archive::Archive::open(&file).is_ok(), "Add accepts it");
        let out = install_file(&state, &mut mods, &file, None, None, None).await;
        assert!(out.is_ok(), "the extension must accept it too: {:?}", out.err());
    }

    #[test]
    fn download_link_to_another_mod_never_resolves_to_the_page_mod() {
        // On mod A's page (4084), a link to mod B's download: the extension
        // sends pageUrl: null and the link as downloadUrl. Host detection
        // alone must not invent an id -- least of all A's.
        let source = resolve_source(None, Some("https://ayakamods.com/mods/other-mod.5555/download"), None).unwrap();
        assert_eq!(source.provider, "ayakamods");
        assert_eq!(source.id, None);

        // When the link itself is B's mod page, it's attributed to B.
        let source = resolve_source(Some("https://ayakamods.com/mods/other-mod.5555/"), None, None).unwrap();
        assert_eq!(source.id.as_deref(), Some("5555"));
    }

    /// Install mod A (ayakamods 4084) from its page, then return the state.
    async fn install_mod_a(dir: &Path) -> (AppState, Vec<Mod>, uuid::Uuid) {
        tokio::fs::create_dir_all(dir.join("mods")).await.unwrap();
        let state = AppState::new(dir.to_path_buf());
        let mut mods = Vec::new();
        let a = dir.join("Mod A.zip");
        make_zip(&a, &[("0123456789abcdef.patch_0", b"A")]);
        let out = install_file(&state, &mut mods, &a, Some(PAGE_URL), None, Some("1.0"))
            .await
            .unwrap();
        (state, mods, out.r#mod.guid())
    }

    #[tokio::test]
    async fn link_to_another_mod_installs_new_and_leaves_the_page_mod_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mut mods, a_guid) = install_mod_a(dir.path()).await;
        let a_dir = mods[0].directory.clone();

        let b = dir.path().join("Mod B.zip");
        make_zip(&b, &[("fedcba9876543210.patch_0", b"B")]);
        let out = install_file(
            &state,
            &mut mods,
            &b,
            None,
            Some("https://ayakamods.com/mods/other-mod.5555/download"),
            None,
        )
        .await
        .unwrap();

        assert!(!out.updated, "a link to mod B must never update mod A");
        assert_ne!(out.r#mod.guid(), a_guid);
        assert_eq!(mods.len(), 2);
        assert_eq!(
            tokio::fs::read(a_dir.join("0123456789abcdef.patch_0")).await.unwrap(),
            b"A",
            "mod A's files must be untouched"
        );
    }

    #[tokio::test]
    async fn unknown_id_never_triggers_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mut mods, a_guid) = install_mod_a(dir.path()).await;

        // Same site, but no parseable mod id anywhere: a forum thread as
        // pageUrl, and a bare CDN-style download link.
        for (i, (page, download)) in [
            (Some("https://ayakamods.com/threads/some-discussion.77/"), None),
            (None, Some("https://ayakamods.com/attachments/file.zip")),
            (None, None),
        ]
        .into_iter()
        .enumerate()
        {
            let f = dir.path().join(format!("unknown-{i}.zip"));
            make_zip(&f, &[("0123456789abcdef.patch_0", b"other")]);
            let out = install_file(&state, &mut mods, &f, page, download, None).await.unwrap();
            assert!(!out.updated, "case {i}: unknown id must install as a new mod");
            assert_ne!(out.r#mod.guid(), a_guid);
        }
        assert_eq!(mods.len(), 4);
    }

    #[tokio::test]
    async fn install_file_maps_path_traversal_to_unsafe_archive() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("mods")).await.unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let mut mods = Vec::new();

        let file = dir.path().join("evil.zip");
        make_zip(&file, &[("../../evil.patch_0", b"evil")]);
        let err = install_file(&state, &mut mods, &file, Some(PAGE_URL), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsafeArchive, "{}", err.message);
        assert!(mods.is_empty());
        // No half-installed "ghost" mod directory may be left behind.
        assert!(!dir.path().join("mods").join("evil").exists());
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
