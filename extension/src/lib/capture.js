/**
 * @file Download-capture matching: deciding whether a browser download is
 * "the mod file" for a site the user just clicked download on (armed
 * capture), or for a site with auto-capture turned on.
 *
 * A download matches a site when:
 *   1. its referrer host, or its url/finalUrl host, is that site's own
 *      domain or one of its known CDN domains, and
 *   2. its filename extension or MIME type is an archive DDMM can install
 *      (zip/7z/rar).
 */
(function initCapture(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  /** Archive file extensions DDMM will install (matches the app's own check). */
  const ARCHIVE_EXTENSIONS = ['zip', '7z', 'rar'];

  /** MIME types browsers commonly report for those archives. */
  const ARCHIVE_MIME_TYPES = [
    'application/zip',
    'application/x-zip-compressed',
    'application/x-7z-compressed',
    'application/x-rar-compressed',
    'application/vnd.rar',
  ];
  // Deliberately excludes application/octet-stream: several CDNs (including
  // modworkshop's storage host) serve *everything* generically with that
  // MIME type, archives and non-archives alike, so it would make the MIME
  // check meaningless. The filename/URL extension check below is the real
  // signal in that case.

  /**
   * Registrable domains a download may legitimately come from for each
   * known site, including CDN hosts that are a *different* registrable
   * domain from the site itself (same-domain subdomains never need listing
   * here -- see {@link hostMatchesSite}). Verified against real responses
   * (see extension/test/fixtures/ and the task report) except where noted.
   * @type {Record<string, string[]>}
   */
  const SITE_HOSTS = {
    ayakamods: ['ayakamods.com'],
    nexus: [
      'nexusmods.com',
      // Confirmed via Nexus Mods' own help docs and Vortex issue reports:
      // premium/CDN downloads are served from nexus-cdn.com, a distinct
      // registrable domain from nexusmods.com.
      'nexus-cdn.com',
    ],
    modworkshop: [
      'modworkshop.net',
      // storage.modworkshop.net is a subdomain, already covered by the
      // suffix match below, but listed for clarity -- confirmed in
      // extension/test/fixtures/modworkshop-mod.html.
    ],
    gamebanana: ['gamebanana.com'],
    github: [
      'github.com',
      // GitHub always redirects release-asset downloads to this CDN.
      'objects.githubusercontent.com',
      'githubusercontent.com',
    ],
  };

  /**
   * @param {string|null|undefined} rawUrl
   * @returns {string|null} Lowercased hostname, or `null` if unparsable.
   */
  function hostOf(rawUrl) {
    if (!rawUrl) return null;
    try {
      return new URL(rawUrl).hostname.toLowerCase();
    } catch {
      return null;
    }
  }

  /**
   * @param {string|null} host
   * @param {string} site - A key of {@link SITE_HOSTS}.
   * @returns {boolean}
   */
  function hostMatchesSite(host, site) {
    if (!host) return false;
    const roots = SITE_HOSTS[site];
    if (!roots) return false;
    return roots.some((allowedRoot) => host === allowedRoot || host.endsWith(`.${allowedRoot}`));
  }

  /**
   * @param {string|null|undefined} filenameOrUrl
   * @returns {boolean}
   */
  function hasArchiveExtension(filenameOrUrl) {
    if (!filenameOrUrl) return false;
    const clean = filenameOrUrl.split(/[?#]/)[0];
    const dot = clean.lastIndexOf('.');
    if (dot === -1) return false;
    const ext = clean.slice(dot + 1).toLowerCase();
    return ARCHIVE_EXTENSIONS.includes(ext);
  }

  /**
   * @param {string|null|undefined} mime
   * @returns {boolean}
   */
  function hasArchiveMime(mime) {
    return Boolean(mime) && ARCHIVE_MIME_TYPES.includes(mime.toLowerCase());
  }

  /**
   * @typedef {object} DownloadLike
   * @property {string} [url]
   * @property {string} [finalUrl]
   * @property {string} [referrer]
   * @property {string} [filename]
   * @property {string} [mime]
   */

   /**
    * @param {DownloadLike} item
    * @returns {boolean} True if the file itself looks like an installable archive.
    */
  function isArchiveDownload(item) {
    return (
      hasArchiveExtension(item.filename) ||
      hasArchiveExtension(item.finalUrl) ||
      hasArchiveExtension(item.url) ||
      hasArchiveMime(item.mime)
    );
  }

  /**
   * @param {DownloadLike} item
   * @param {string} site
   * @returns {boolean} True if the download's referrer or (final) URL host
   *   belongs to `site`.
   */
  function isFromSite(item, site) {
    return (
      hostMatchesSite(hostOf(item.referrer), site) ||
      hostMatchesSite(hostOf(item.finalUrl), site) ||
      hostMatchesSite(hostOf(item.url), site)
    );
  }

  /** Default arm window: 10 minutes, per the task spec. */
  const DEFAULT_ARM_WINDOW_MS = 10 * 60 * 1000;

  /**
   * Tracks "the next download from this site is the one the user just
   * clicked download for" -- a one-shot, time-boxed arm per (site, tab).
   * Used for sites without a direct file link (AyakaMods login-gated files,
   * Nexus's manual "Slow download" click).
   */
  class CaptureRegistry {
    /**
     * @param {object} [opts]
     * @param {() => number} [opts.now] - Clock, injectable for tests.
     * @param {number} [opts.windowMs]
     */
    constructor(opts = {}) {
      this._now = opts.now || (() => Date.now());
      this._windowMs = opts.windowMs || DEFAULT_ARM_WINDOW_MS;
      /** @type {Map<string, {tabId: number|null, expiresAt: number, pageUrl: string|null, pageVersion: string|null}>} */
      this._armed = new Map();
    }

    /**
     * Arm capture for `site`. A second arm for the same site replaces the
     * first (fresh 10-minute window).
     * @param {string} site
     * @param {{tabId?: number|null, pageUrl?: string|null}} [opts]
     */
    arm(site, opts = {}) {
      this._armed.set(site, {
        tabId: opts.tabId ?? null,
        pageUrl: opts.pageUrl ?? null,
        pageVersion: opts.pageVersion ?? null,
        expiresAt: this._now() + this._windowMs,
      });
    }

    /** @param {string} site */
    disarm(site) {
      this._armed.delete(site);
    }

    _pruneExpired() {
      const now = this._now();
      for (const [site, entry] of this._armed) {
        if (entry.expiresAt <= now) this._armed.delete(site);
      }
    }

    /** @param {string} site @returns {boolean} */
    isArmed(site) {
      this._pruneExpired();
      return this._armed.has(site);
    }

    /**
     * Check a completed download against every currently-armed site. Matches
     * are one-shot: a hit disarms that site.
     * @param {DownloadLike} item
     * @returns {{site: string, pageUrl: string|null, tabId: number|null}|null}
     */
    match(item) {
      this._pruneExpired();
      if (!isArchiveDownload(item)) return null;
      for (const [site, entry] of this._armed) {
        if (isFromSite(item, site)) {
          this._armed.delete(site);
          return { site, pageUrl: entry.pageUrl, pageVersion: entry.pageVersion, tabId: entry.tabId };
        }
      }
      return null;
    }
  }

  DDMM.capture = {
    SITE_HOSTS,
    ARCHIVE_EXTENSIONS,
    ARCHIVE_MIME_TYPES,
    hostMatchesSite,
    hasArchiveExtension,
    hasArchiveMime,
    isArchiveDownload,
    isFromSite,
    CaptureRegistry,
    DEFAULT_ARM_WINDOW_MS,
  };
})(typeof globalThis !== 'undefined' ? globalThis : this);
