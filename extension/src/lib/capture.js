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
      // registrable domain from nexusmods.com (e.g.
      // supporter-files.nexus-cdn.com). Free downloads come from
      // cf-files.nexusmods.com, already covered as a subdomain.
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
   * How far back a click on "Install with DDMM" may reach for a download
   * that already finished. A userscript or download manager can start the
   * file the moment the page opens (without Nexus's countdown), so it may
   * be done before the user gets to our button.
   */
  const RECENT_WINDOW_MS = 5 * 60 * 1000;

  /** At most this many finished, unclaimed downloads are remembered. */
  const MAX_RECENT = 20;

  /**
   * Whether an already-finished download is the mod of `pageUrl`: the
   * download's own URL names that mod (a Nexus file URL carries the mod id),
   * or -- when its URL names no mod at all -- its referrer is a page of
   * that mod. Anything unsure is *not* claimed: an earlier download is only
   * ever installed for the page it provably belongs to.
   * @param {DownloadLike} item
   * @param {string|null} pageUrl
   * @returns {boolean}
   */
  function downloadBelongsToPage(item, pageUrl) {
    const sources = root.DDMM && root.DDMM.sources;
    if (!sources || !pageUrl) return false;
    const pageSource = sources.sourceFromPageUrl(pageUrl);
    if (!pageSource) return false;
    const link = sources.modFromDownloadUrl(item.finalUrl) || sources.modFromDownloadUrl(item.url);
    if (link) return sources.isSameMod(link.source, pageSource);
    const referrerSource = item.referrer ? sources.sourceFromPageUrl(item.referrer) : null;
    return sources.isSameMod(referrerSource, pageSource);
  }

  /**
   * Tracks "the next download from this site is the one the user just
   * clicked download for" -- a one-shot, time-boxed arm per site. Used for
   * sites without a direct file link (AyakaMods login-gated files, Nexus's
   * manual download). The arm doesn't care *how* the download starts or in
   * which tab: whatever archive next arrives from the site (its pages or
   * CDN hosts) is the one. It also remembers recent unclaimed downloads, so
   * a file that finished *before* the click (a userscript or download
   * manager was faster than the user) is still picked up.
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
      /**
       * Finished archive downloads from a known site that nothing claimed
       * yet (not armed, auto-capture off), newest last.
       * @type {Array<{item: DownloadLike, site: string, at: number}>}
       */
      this._recent = [];
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

    /**
     * Remember a finished download nothing claimed, so a click on "Install
     * with DDMM" shortly afterwards can still pick it up. Only archives from
     * a known site are kept.
     * @param {DownloadLike} item
     */
    remember(item) {
      if (!isArchiveDownload(item)) return;
      const site = Object.keys(SITE_HOSTS).find((s) => isFromSite(item, s));
      if (!site) return;
      this._pruneRecent();
      this._recent.push({ item, site, at: this._now() });
      if (this._recent.length > MAX_RECENT) this._recent.shift();
    }

    _pruneRecent() {
      const cutoff = this._now() - RECENT_WINDOW_MS;
      this._recent = this._recent.filter((r) => r.at > cutoff);
    }

    /**
     * Take (one-shot) the newest recent download from `site` that belongs to
     * the mod at `pageUrl` (see {@link downloadBelongsToPage}).
     * @param {string} site
     * @param {string|null} pageUrl
     * @returns {DownloadLike|null}
     */
    claimRecent(site, pageUrl) {
      this._pruneRecent();
      for (let i = this._recent.length - 1; i >= 0; i -= 1) {
        const entry = this._recent[i];
        if (entry.site === site && downloadBelongsToPage(entry.item, pageUrl)) {
          this._recent.splice(i, 1);
          return entry.item;
        }
      }
      return null;
    }

    /** Forget every remembered download. */
    clearRecent() {
      this._recent = [];
    }

    /**
     * Plain-JSON snapshot, so a Chrome MV3 service worker that gets shut
     * down while the user waits on a download (it does, after ~30 s idle)
     * can pick up where it left off.
     * @returns {{armed: Array<[string, object]>, recent: Array<object>}}
     */
    serialize() {
      this._pruneExpired();
      this._pruneRecent();
      return { armed: [...this._armed.entries()], recent: this._recent.slice() };
    }

    /**
     * Merge a {@link serialize} snapshot back in. Entries already present
     * (armed since the snapshot) win.
     * @param {{armed?: Array<[string, object]>, recent?: Array<object>}|null|undefined} snapshot
     */
    restore(snapshot) {
      if (!snapshot) return;
      for (const [site, entry] of snapshot.armed || []) {
        if (!this._armed.has(site) && SITE_HOSTS[site]) this._armed.set(site, entry);
      }
      const known = new Set(this._recent.map((r) => r.item && r.item.id));
      for (const r of snapshot.recent || []) {
        if (r && r.item && !known.has(r.item.id)) this._recent.push(r);
      }
      this._recent.sort((a, b) => a.at - b.at);
      this._pruneExpired();
      this._pruneRecent();
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
     * @returns {{site: string, pageUrl: string|null, pageVersion: string|null, tabId: number|null}|null}
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
    downloadBelongsToPage,
    CaptureRegistry,
    DEFAULT_ARM_WINDOW_MS,
    RECENT_WINDOW_MS,
  };
})(typeof globalThis !== 'undefined' ? globalThis : this);
