/**
 * @file Mod-page URL parsing, mirroring the desktop app's
 * `source_from_page_url` (src-tauri/src/sources.rs) exactly, so the browser
 * extension and DDMM agree on what a mod page URL means for every site.
 *
 * No provider is privileged: this module is a straight port of the Rust
 * logic, kept in lockstep by the shared unit test fixtures.
 */
(function initSources(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  /**
   * @typedef {object} Source
   * @property {string} provider - Provider key (`"ayakamods"`, `"nexus"`, `"modworkshop"`, `"gamebanana"`, `"github"`, or `"url"`).
   * @property {string|null} id - Provider-specific id (numeric string, or `"<owner>/<repo>"` for GitHub).
   */

  /**
   * @param {string} s
   * @returns {boolean} True if `s` is non-empty and every character is an ASCII digit.
   */
  function isNumeric(s) {
    return s.length > 0 && /^[0-9]+$/.test(s);
  }

  /**
   * Split a URL's path into non-empty segments, the same way
   * `Url::path_segments()` does in the Rust implementation.
   * @param {URL} url
   * @returns {string[]}
   */
  function pathSegments(url) {
    return url.pathname.split('/').filter((s) => s.length > 0);
  }

  /**
   * Extract a structured {@link Source} from a mod *page* URL (as opposed to
   * a direct download link), for the sites DDMM knows the page-URL shape of.
   * Returns `null` for anything else.
   *
   * Query strings and fragments are ignored, trailing slashes don't matter,
   * and non-numeric/garbage ids are rejected rather than guessed at -- this
   * must match `source_from_page_url` in src-tauri/src/sources.rs.
   *
   * @param {string} rawUrl
   * @returns {Source|null}
   */
  function sourceFromPageUrl(rawUrl) {
    let parsed;
    try {
      parsed = new URL(rawUrl);
    } catch {
      return null;
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return null;
    }

    let host = parsed.hostname.toLowerCase();
    if (host.startsWith('www.')) {
      host = host.slice(4);
    }
    const segments = pathSegments(parsed);

    switch (host) {
      case 'ayakamods.com': {
        // /mods/<slug>.<id>/ or /mods/<id>/
        if (segments[0] !== 'mods' || segments.length < 2) return null;
        const last = segments[1];
        const id = last.includes('.') ? last.split('.').pop() : last;
        return isNumeric(id) ? { provider: 'ayakamods', id } : null;
      }
      case 'nexusmods.com': {
        // /helldivers2/mods/<id>
        if (segments[0] !== 'helldivers2' || segments[1] !== 'mods') return null;
        const id = segments[2];
        return id && isNumeric(id) ? { provider: 'nexus', id } : null;
      }
      case 'modworkshop.net': {
        if (segments[0] !== 'mod' || segments.length < 2) return null;
        const id = segments[1];
        return id ? { provider: 'modworkshop', id } : null;
      }
      case 'gamebanana.com': {
        if (segments[0] !== 'mods' || segments.length < 2) return null;
        const id = segments[1];
        return isNumeric(id) ? { provider: 'gamebanana', id } : null;
      }
      case 'github.com': {
        const owner = segments[0];
        const repo = segments[1];
        return owner && repo ? { provider: 'github', id: `${owner}/${repo}` } : null;
      }
      default:
        return null;
    }
  }

  /**
   * Best-effort host extraction without a full URL parse, mirroring
   * `extract_host` in sources.rs. Prefer {@link sourceFromPageUrl} or the
   * `URL` constructor directly when a full parse is available; this exists
   * only where the Rust reference implementation uses the lightweight path.
   * @param {string} url
   * @returns {string|null}
   */
  function extractHost(url) {
    const afterScheme = url.split('://')[1];
    if (!afterScheme) return null;
    const authority = afterScheme.split(/[/?#]/)[0];
    if (!authority) return null;
    const hostPort = authority.split('@').pop();
    const host = hostPort.split(':')[0];
    return host ? host.toLowerCase() : null;
  }

  /**
   * Guess a provider id from a raw URL's host, mirroring `provider_from_url`
   * in sources.rs.
   * @param {string} url
   * @returns {string}
   */
  function providerFromUrl(url) {
    const host = extractHost(url);
    if (!host) return 'url';

    if (host === 'nexusmods.com' || host.endsWith('.nexusmods.com')) return 'nexus';
    if (host === 'modworkshop.net' || host.endsWith('.modworkshop.net')) return 'modworkshop';
    if (
      host === 'github.com' ||
      host.endsWith('.github.com') ||
      host === 'objects.githubusercontent.com' ||
      host.endsWith('.objects.githubusercontent.com')
    ) {
      return 'github';
    }
    if (host === 'gamebanana.com' || host.endsWith('.gamebanana.com')) return 'gamebanana';
    if (host === 'ayakamods.com' || host.endsWith('.ayakamods.com')) return 'ayakamods';
    return 'url';
  }

  DDMM.sources = { sourceFromPageUrl, providerFromUrl, extractHost, isNumeric };
})(typeof globalThis !== 'undefined' ? globalThis : this);
