//! AyakaMods: no public API, but every mod page carries a JSON-LD
//! `SoftwareApplication` block whose `softwareVersion` is the current
//! version. Downloads need a login, so updates go through the browser.

use super::fetch_capped;

pub const HOST: &str = "ayakamods.com";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AyakaModsMetadata {
    pub name: Option<String>,
    pub latest_version: Option<String>,
    pub modified: Option<String>,
}

pub(crate) async fn fetch_ayakamods_metadata(
    client: &reqwest::Client,
    id: &str,
) -> anyhow::Result<Option<AyakaModsMetadata>> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        anyhow::bail!("\"{id}\" isn't an AyakaMods mod id");
    }
    let url = format!("https://{HOST}/mods/{id}/");
    let (html, _) = fetch_capped(client, &url).await?;
    Ok(parse_ayakamods_page(&html))
}

/// Extract the mod's `SoftwareApplication` JSON-LD block from a saved (or
/// freshly-fetched) AyakaMods mod page. `None` for anything that doesn't
/// parse -- missing/malformed JSON-LD is not an error, it's just
/// "unknown" to the caller.
pub fn parse_ayakamods_page(html: &str) -> Option<AyakaModsMetadata> {
    for block in extract_ld_json_blocks(html) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&block) else {
            continue;
        };
        if let Some(app) = find_software_application(&value) {
            return Some(AyakaModsMetadata {
                name: app.get("name").and_then(|v| v.as_str()).map(str::to_string),
                latest_version: app
                    .get("softwareVersion")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                modified: app
                    .get("dateModified")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }
    None
}

/// Scan `html` for `<script type="application/ld+json">...</script>`
/// blocks. Deliberately simple (no HTML parser crate, just `regex` -- an
/// existing dependency): good enough for extracting a well-formed script
/// tag's contents, not a general HTML parser.
fn extract_ld_json_blocks(html: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(?is)<script[^>]*type\s*=\s*"application/ld\+json"[^>]*>(.*?)</script>"#)
            .unwrap()
    });
    re.captures_iter(html)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Depth-first search through a JSON-LD value (a plain object, an array of
/// objects/graphs, or an object with an `@graph` array) for the first node
/// whose `@type` is `SoftwareApplication`.
fn find_software_application(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => items.iter().find_map(find_software_application),
        serde_json::Value::Object(map) => {
            if map.get("@type").and_then(|v| v.as_str()) == Some("SoftwareApplication") {
                return Some(value);
            }
            map.get("@graph").and_then(find_software_application)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/ayakamods_mod_page.html");

    #[test]
    fn parses_real_ayakamods_fixture() {
        let meta = parse_ayakamods_page(FIXTURE).expect("should find JSON-LD");
        assert_eq!(meta.name.as_deref(), Some("HD2 Auto Reload"));
        assert_eq!(meta.latest_version.as_deref(), Some("2026-09-24"));
        assert!(meta.modified.is_some());
    }

    #[test]
    fn missing_ld_json_is_none_not_error() {
        assert!(parse_ayakamods_page("<html><body>no json-ld here</body></html>").is_none());
    }

    #[test]
    fn malformed_ld_json_is_none_not_error() {
        let html = r#"<script type="application/ld+json">{ not: valid json </script>"#;
        assert!(parse_ayakamods_page(html).is_none());
    }

    #[test]
    fn ld_json_without_software_application_is_none() {
        let html = r#"<script type="application/ld+json">[{"@type":"Organization","name":"Someone"}]</script>"#;
        assert!(parse_ayakamods_page(html).is_none());
    }

    #[test]
    fn ld_json_graph_form_is_found() {
        let html = r#"<script type="application/ld+json">
            {"@graph": [{"@type":"Organization"}, {"@type":"SoftwareApplication","name":"Graph Mod","softwareVersion":"1.2.3"}]}
        </script>"#;
        let meta = parse_ayakamods_page(html).expect("should find nested graph entry");
        assert_eq!(meta.name.as_deref(), Some("Graph Mod"));
        assert_eq!(meta.latest_version.as_deref(), Some("1.2.3"));
    }

    /// Real network smoke test against the live page. `#[ignore]`d so the
    /// normal suite never depends on network access.
    #[tokio::test]
    #[ignore]
    async fn fetches_and_parses_the_real_page() {
        let client = super::super::build_client().unwrap();
        let meta = fetch_ayakamods_metadata(&client, "hd2-auto-reload.4084")
            .await
            .unwrap()
            .expect("should parse JSON-LD from the live page");
        assert!(meta.latest_version.is_some());
    }
}
