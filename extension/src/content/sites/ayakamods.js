/**
 * @file AyakaMods content script.
 *
 * Real DOM verified against a live fetch of
 * https://ayakamods.com/mods/hd2-auto-reload.4084/ (XenForo resource
 * manager). See extension/test/fixtures/ayakamods-mod.html.
 *
 * Findings:
 *   - The download button lives in the first `.modSidebarGroup--buttons`
 *     sidebar block. For a guest (not logged in) it's a disabled
 *     `<span class="button ... is-disabled">Log in or sign up to
 *     download</span>` -- there's no direct file link at all until you're
 *     logged in, so DDMM must arm capture and let the user click AyakaMods'
 *     own (real) download control themselves once they're logged in.
 *   - Version comes from the page's `SoftwareApplication` JSON-LD
 *     (`softwareVersion`), not from any visible text.
 */
/* global DDMM */
(function initAyakaMods() {
  'use strict';

  /** @param {Document} doc @returns {Element|null} */
  function findInsertionPoint(doc) {
    return doc.querySelector('.modSidebarGroup--buttons');
  }

  /**
   * Only returns a URL when AyakaMods has rendered a *real* anchor (i.e. the
   * user is logged in and a file is attached) rather than the disabled
   * "Log in or sign up to download" placeholder, which XenForo renders as a
   * `<span>`, never an `<a>`.
   * @param {Document} doc
   * @returns {string|null}
   */
  function findDirectDownloadUrl(doc) {
    const group = findInsertionPoint(doc);
    if (!group) return null;
    const link = group.querySelector('a[href]');
    return link ? link.href : null;
  }

  /** @param {Document} doc @returns {string|null} */
  function scrapeVersion(doc) {
    const scripts = doc.querySelectorAll('script[type="application/ld+json"]');
    for (const script of scripts) {
      try {
        const data = JSON.parse(script.textContent);
        const entries = Array.isArray(data) ? data : [data];
        for (const entry of entries) {
          if (entry && entry['@type'] === 'SoftwareApplication' && entry.softwareVersion) {
            return String(entry.softwareVersion);
          }
        }
      } catch {
        // Not every ld+json block on the page is ours to parse; skip malformed ones.
      }
    }
    return null;
  }

  const adapter = { name: 'ayakamods', findInsertionPoint, findDirectDownloadUrl, scrapeVersion };
  DDMM.siteAdapters = DDMM.siteAdapters || {};
  DDMM.siteAdapters.ayakamods = adapter;

  // Guarded so this file can be loaded in isolation (unit tests exercise
  // the finder functions above directly via DDMM.siteAdapters.ayakamods)
  // without also loading content/common.js.
  if (DDMM.content) DDMM.content.runSiteAdapter(adapter);
})();
