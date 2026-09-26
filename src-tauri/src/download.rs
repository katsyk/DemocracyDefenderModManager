//! Downloading a mod archive from a direct URL.
//!
//! This deliberately does not care which site the URL points at: any
//! `https://` link that ends up serving a zip/7z/rar archive works, whether
//! that's Nexus, ModWorkshop, GitHub, GameBanana, or a personal file host.
//! Sites that gate their actual download behind a login or JavaScript (like
//! Nexus's mod page itself, as opposed to a direct CDN link) will simply
//! serve back HTML, which is rejected by the archive-format check below.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

/// Hard cap on how much a single download is allowed to be, enforced both
/// from `Content-Length` (when present) and by counting streamed bytes.
pub const MAX_DOWNLOAD_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Directory (under DDMM's data dir) that URL downloads are staged in; see
/// [`download_archive`].
pub const STAGING_DIRECTORY: &str = ".downloads";

/// `url` with its query string and fragment removed, for logging: direct
/// CDN links carry short-lived access tokens there, and users attach their
/// logs to bug reports.
pub fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "<unparseable URL>".to_string(),
    }
}

/// A successfully downloaded and identified archive, staged under its own
/// throwaway directory. Callers are responsible for removing `temp_dir` once
/// they are done with `path` (success or failure).
pub struct DownloadedArchive {
    pub path: PathBuf,
    pub temp_dir: PathBuf,
}

/// Download `url` to a fresh directory under `staging_root`, sniff/confirm
/// it is a zip/7z/rar archive, and return its staged path.
///
/// `staging_root` is normally a directory inside DDMM's own data dir rather
/// than `std::env::temp_dir()`: on Arch/CachyOS (and most systemd distros)
/// `/tmp` is a RAM-backed tmpfs that is often much smaller than the disk,
/// so a large mod could fail there with "No space left on device", and it
/// keeps the staged archive on the same filesystem as `mods/`.
///
/// On any error the staging directory is cleaned up before returning.
pub async fn download_archive(url: &str, staging_root: &Path) -> anyhow::Result<DownloadedArchive> {
    download_archive_with_progress(url, staging_root, |_, _| {}).await
}

/// [`download_archive`], calling `progress(downloaded_bytes, total_bytes)`
/// as the body streams in (`total` is `None` when the server didn't say).
pub async fn download_archive_with_progress(
    url: &str,
    staging_root: &Path,
    mut progress: impl FnMut(u64, Option<u64>) + Send,
) -> anyhow::Result<DownloadedArchive> {
    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL: {}", e))?;
    if parsed.scheme() != "https" {
        anyhow::bail!("only https:// URLs are supported");
    }

    let temp_dir = staging_root.join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .with_context(|| format!("failed to create download staging directory {:?}", temp_dir))?;

    match download_archive_into(&parsed, &temp_dir, &mut progress).await {
        Ok(path) => Ok(DownloadedArchive { path, temp_dir }),
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            Err(e)
        }
    }
}

async fn download_archive_into(
    url: &reqwest::Url,
    temp_dir: &Path,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> anyhow::Result<PathBuf> {
    let client = reqwest::Client::builder()
        .user_agent(crate::providers::user_agent())
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(10 * 60))
        .connect_timeout(Duration::from_secs(20))
        .build()?;

    // `without_url()`: reqwest errors embed the full URL, and direct CDN
    // links (e.g. Nexus) carry short-lived access tokens in the query
    // string. Errors are now shown to the user and written to the log, so
    // name the host instead.
    let host = url.host_str().unwrap_or("the server").to_string();
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("couldn't reach {}: {}", host, e.without_url()))?;
    let response = response
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("{} refused the download: {}", host, e.without_url()))?;

    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_SIZE {
            anyhow::bail!(
                "download is {} bytes, which exceeds the {} byte limit",
                len,
                MAX_DOWNLOAD_SIZE
            );
        }
    }

    let final_url = response.url().clone();
    let content_disposition_name = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_disposition_filename);

    let staging_path = temp_dir.join("download.part");
    let mut file = tokio::fs::File::create(&staging_path)
        .await
        .with_context(|| format!("failed to create {:?}", staging_path))?;
    let expected_len = response.content_length();
    let mut stream = response.bytes_stream();
    let mut total: u64 = 0;
    progress(0, expected_len);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("download from {} failed: {}", host, e.without_url()))?;
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_SIZE {
            anyhow::bail!(
                "download exceeded the {} byte limit while streaming",
                MAX_DOWNLOAD_SIZE
            );
        }
        file.write_all(&chunk)
            .await
            .with_context(|| format!("failed to write the download to {:?}", staging_path))?;
        progress(total, expected_len);
    }
    file.flush().await?;
    drop(file);

    let raw_name = content_disposition_name
        .or_else(|| last_path_segment(&final_url))
        .unwrap_or_else(|| "download".to_string());
    let mut filename = sanitize_filename(&raw_name);

    // What arrived, by content: a site that wants a login or shows a
    // "please wait" page sends HTML even for a URL ending in `.zip`.
    let header = {
        use tokio::io::AsyncReadExt;
        let mut header = Vec::with_capacity(crate::archive::SNIFF_LEN);
        tokio::fs::File::open(&staging_path)
            .await?
            .take(crate::archive::SNIFF_LEN as u64)
            .read_to_end(&mut header)
            .await?;
        header
    };
    match crate::archive::sniff(&header) {
        crate::archive::Sniffed::Archive(format) => {
            if !has_supported_archive_extension(&filename) {
                filename = format!("{}.{}", filename, format.name());
            }
        }
        crate::archive::Sniffed::NotArchive(what) => {
            anyhow::bail!("downloaded file is not a supported archive (zip/7z/rar): {host} sent {what}")
        }
        crate::archive::Sniffed::Empty => {
            anyhow::bail!("downloaded file is not a supported archive (zip/7z/rar): {host} sent an empty file")
        }
        // Possibly a zip with something in front of it; opening it will tell.
        crate::archive::Sniffed::Unknown | crate::archive::Sniffed::Zeros if filename.to_ascii_lowercase().ends_with(".zip") => {}
        crate::archive::Sniffed::Unknown | crate::archive::Sniffed::Zeros => {
            anyhow::bail!("downloaded file is not a supported archive (zip/7z/rar)")
        }
    }

    let final_path = temp_dir.join(&filename);
    crate::fs_util::move_path(&staging_path, &final_path).await?;

    Ok(final_path)
}

/// Turn a raw (possibly attacker-controlled) filename hint into a bare file
/// name safe to join onto a directory: no path separators, no `..`/`.`
/// traversal tricks, no control characters.
fn sanitize_filename(raw: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| raw.to_string());

    let base = decoded
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or("");

    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();

    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        return "download".to_string();
    }

    cleaned.to_string()
}

fn has_supported_archive_extension(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower.ends_with(".zip") || lower.ends_with(".7z") || lower.ends_with(".rar")
}

fn last_path_segment(url: &reqwest::Url) -> Option<String> {
    let last = url.path_segments()?.rfind(|s: &&str| !s.is_empty())?;
    let decoded = percent_encoding::percent_decode_str(last).decode_utf8_lossy();
    Some(decoded.into_owned())
}

fn parse_content_disposition_filename(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.split(';').map(str::trim).collect();

    for part in &parts {
        if let Some(rest) = part.strip_prefix("filename*=") {
            if let Some(idx) = rest.find("''") {
                let encoded = &rest[idx + 2..];
                let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy();
                if !decoded.is_empty() {
                    return Some(decoded.into_owned());
                }
            }
        }
    }

    for part in &parts {
        if let Some(rest) = part.strip_prefix("filename=") {
            let trimmed = rest.trim().trim_matches('"');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_directory_traversal() {
        assert_eq!(sanitize_filename("../../evil.zip"), "evil.zip");
        assert_eq!(sanitize_filename("..\\..\\evil.zip"), "evil.zip");
        assert_eq!(sanitize_filename("/etc/passwd"), "passwd");
    }

    #[test]
    fn sanitize_filename_rejects_bare_dots() {
        assert_eq!(sanitize_filename(".."), "download");
        assert_eq!(sanitize_filename("."), "download");
        assert_eq!(sanitize_filename(""), "download");
    }

    #[test]
    fn sanitize_filename_strips_control_characters() {
        assert_eq!(sanitize_filename("mod\n.zip"), "mod.zip");
    }

    #[test]
    fn sanitize_filename_keeps_plain_names() {
        assert_eq!(sanitize_filename("cool-mod_v2.zip"), "cool-mod_v2.zip");
    }

    fn sniff_archive_extension(header: &[u8]) -> Option<&'static str> {
        match crate::archive::sniff(header) {
            crate::archive::Sniffed::Archive(format) => Some(format.name()),
            _ => None,
        }
    }

    #[test]
    fn sniff_archive_extension_detects_zip() {
        assert_eq!(
            sniff_archive_extension(b"PK\x03\x04rest-of-file"),
            Some("zip")
        );
    }

    #[test]
    fn sniff_archive_extension_detects_7z() {
        let header = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00, 0x00];
        assert_eq!(sniff_archive_extension(&header), Some("7z"));
    }

    #[test]
    fn sniff_archive_extension_detects_rar() {
        assert_eq!(
            sniff_archive_extension(b"Rar!\x1a\x07\x01\x00"),
            Some("rar")
        );
    }

    #[test]
    fn sniff_archive_extension_rejects_html() {
        assert_eq!(sniff_archive_extension(b"<!DOCTYPE html>"), None);
    }

    #[test]
    fn has_supported_archive_extension_is_case_insensitive() {
        assert!(has_supported_archive_extension("mod.ZIP"));
        assert!(has_supported_archive_extension("mod.7z"));
        assert!(has_supported_archive_extension("mod.RAR"));
        assert!(!has_supported_archive_extension("mod.exe"));
    }

    #[test]
    fn parse_content_disposition_simple() {
        assert_eq!(
            parse_content_disposition_filename("attachment; filename=\"cool-mod.zip\""),
            Some("cool-mod.zip".to_string())
        );
    }

    #[test]
    fn parse_content_disposition_rfc5987() {
        assert_eq!(
            parse_content_disposition_filename(
                "attachment; filename*=UTF-8''cool%20mod.zip"
            ),
            Some("cool mod.zip".to_string())
        );
    }

    #[test]
    fn parse_content_disposition_missing_returns_none() {
        assert_eq!(parse_content_disposition_filename("inline"), None);
    }

    #[test]
    fn redact_url_strips_query_and_fragment() {
        assert_eq!(
            redact_url("https://cdn.example.com/files/mod.zip?md5=abc&expires=123#frag"),
            "https://cdn.example.com/files/mod.zip"
        );
        assert_eq!(redact_url("not a url"), "<unparseable URL>");
    }

    #[test]
    fn last_path_segment_decodes_percent_encoding() {
        let url = reqwest::Url::parse("https://example.com/files/cool%20mod.zip").unwrap();
        assert_eq!(last_path_segment(&url), Some("cool mod.zip".to_string()));
    }

    /// Real network smoke test for the whole pipeline. Intentionally
    /// `#[ignore]`d so the normal test suite never depends on network
    /// access; run explicitly with `cargo test -- --ignored` when you want
    /// to exercise it.
    #[tokio::test]
    #[ignore]
    async fn downloads_a_real_small_zip() {
        let staging = tempfile::tempdir().unwrap();
        let result = download_archive(
            "https://github.com/octocat/Hello-World/archive/refs/heads/master.zip",
            staging.path(),
        )
        .await
        .expect("download should succeed");

        assert!(result.path.exists());
        assert!(has_supported_archive_extension(
            result.path.to_str().unwrap()
        ));

        let _ = tokio::fs::remove_dir_all(&result.temp_dir).await;
    }
}
