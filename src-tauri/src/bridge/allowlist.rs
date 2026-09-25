//! The extension-origin allowlist and the registrable-domain ("eTLD+1")
//! logic behind per-site consent (`BridgeAllowedSites`).

/// Chrome/Edge/Brave/Opera/Vivaldi extension ID (from the unpacked
/// manifest's `key`) and the Firefox extension ID
/// (`browser_specific_settings.gecko.id`) -- the only two callers the app
/// side of the bridge ever trusts. Kept in one place so both the origin
/// check and the native-messaging manifests reference the same values.
pub const CHROME_EXTENSION_ID: &str = "inomhciahaeeefhgdkiaabdponcfdane";
pub const FIREFOX_EXTENSION_ID: &str = "ddmm@katsyk.github.io";

/// The exact `origin` values the app accepts on a forwarded request -- the
/// host relay sets this field itself (see `host::relay`), so trusting it
/// here means trusting the host, not the browser's own claim.
fn allowed_origins() -> [String; 1] {
    [format!("chrome-extension://{CHROME_EXTENSION_ID}/")]
}

/// Whether `origin` (as set by the host relay) is one this app accepts.
/// The Firefox case is the bare extension ID, not a URL -- Firefox's
/// native-messaging host has no equivalent of `chrome-extension://…/`.
pub fn is_allowed_origin(origin: &str) -> bool {
    origin == FIREFOX_EXTENSION_ID || allowed_origins().iter().any(|o| o == origin)
}

/// Well-known suffixes with a public second-level registration point (a
/// tiny, hand-maintained stand-in for a full public-suffix list -- see
/// `registrable_domain`'s doc comment for why that's good enough here).
const TWO_LABEL_PUBLIC_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "gov.uk", "ac.uk",
    "co.jp", "ne.jp", "or.jp",
    "com.au", "net.au", "org.au",
    "com.br", "co.nz", "co.in", "co.za",
];

/// A minimal "registrable domain" (informally, eTLD+1) extraction: the
/// last two labels of a hostname, or the last three when the last two
/// labels match a known two-label public suffix (`co.uk`, `com.au`, ...).
///
/// This is deliberately not a full [Public Suffix List](https://publicsuffix.org/)
/// implementation (the `psl`/`publicsuffix` crates are hundreds of KB of
/// generated tables) -- consent is scoped per mod site, and every mod site
/// this app knows about (ayakamods.com, nexusmods.com, modworkshop.net,
/// gamebanana.com, github.com, plus arbitrary direct-download hosts) sits
/// on an ordinary `example.com`-shaped domain. The short list above covers
/// the common two-label-suffix countries a personal file host might use;
/// anything else falls back to "last two labels", which is right for the
/// vast majority of real hostnames and, worst case, is merely a slightly
/// too-broad (never too-narrow in a way that leaks unrelated sites into
/// the same consent) grouping.
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();

    if labels.len() <= 2 {
        return host;
    }

    let last_two = labels[labels.len() - 2..].join(".");
    if TWO_LABEL_PUBLIC_SUFFIXES.contains(&last_two.as_str()) && labels.len() >= 3 {
        labels[labels.len() - 3..].join(".")
    } else {
        last_two
    }
}

/// `registrable_domain`, applied to a URL string instead of a bare host.
/// `None` if the URL doesn't parse or has no host (e.g. isn't `http(s)`).
pub fn registrable_domain_of_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(registrable_domain(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_origin_is_allowed() {
        assert!(is_allowed_origin("chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/"));
    }

    #[test]
    fn firefox_origin_is_allowed() {
        assert!(is_allowed_origin("ddmm@katsyk.github.io"));
    }

    #[test]
    fn unknown_chrome_extension_id_is_rejected() {
        assert!(!is_allowed_origin("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"));
    }

    #[test]
    fn unrelated_origin_is_rejected() {
        assert!(!is_allowed_origin("https://evil.example.com"));
        assert!(!is_allowed_origin(""));
    }

    #[test]
    fn registrable_domain_simple_host() {
        assert_eq!(registrable_domain("ayakamods.com"), "ayakamods.com");
        assert_eq!(registrable_domain("www.ayakamods.com"), "ayakamods.com");
        assert_eq!(registrable_domain("cdn.assets.nexusmods.com"), "nexusmods.com");
    }

    #[test]
    fn registrable_domain_two_label_public_suffix() {
        assert_eq!(registrable_domain("mods.example.co.uk"), "example.co.uk");
        assert_eq!(registrable_domain("example.co.uk"), "example.co.uk");
    }

    #[test]
    fn registrable_domain_bare_host_unchanged() {
        assert_eq!(registrable_domain("localhost"), "localhost");
    }

    #[test]
    fn registrable_domain_of_url_extracts_host() {
        assert_eq!(
            registrable_domain_of_url("https://www.ayakamods.com/mods/hd2-auto-reload.4084/"),
            Some("ayakamods.com".to_string())
        );
    }

    #[test]
    fn registrable_domain_of_url_none_for_unparseable() {
        assert_eq!(registrable_domain_of_url("not a url"), None);
    }
}
