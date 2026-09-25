/**
 * @file GameBanana content script.
 *
 * Real DOM verified against a live fetch of
 * https://gamebanana.com/mods/641397. See
 * extension/test/fixtures/gamebanana-mod.html.
 *
 * Findings:
 *   - The page ships `<module data-module="Files"></module>` -- a real,
 *     stable, empty placeholder that GameBanana's own JS fills with the
 *     actual file list (and download links) client-side after load. A curl
 *     fetch never sees the links, but a real browser (where this content
 *     script runs, after the site's own scripts have had time to run) does,
 *     so this module element is used as both the insertion point and the
 *     download-link search root. Public reporting on GameBanana's own
 *     download flow (see the task report) confirms `/mods/download/<id>`
 *     as the file-link path shape.
 *   - No page-visible version string was found in the static HTML or the
 *     `CreativeWork` JSON-LD.
 */
/* global DDMM */
(function initGameBanana() {
  'use strict';

  /** @param {Document} doc @returns {Element|null} */
  function findFilesModule(doc) {
    return doc.querySelector('module[data-module="Files"]');
  }

  /** @param {Document} doc @returns {Element|null} */
  function findInsertionPoint(doc) {
    return findFilesModule(doc);
  }

  /** @param {Document} doc @returns {string|null} */
  function findDirectDownloadUrl(doc) {
    const module = findFilesModule(doc);
    if (!module) return null;
    const anchors = Array.from(module.querySelectorAll('a[href]'));
    const byPath = anchors.find((a) => a.href.includes('/mods/download/'));
    if (byPath) return byPath.href;
    const byExtension = anchors.find((a) => DDMM.capture.hasArchiveExtension(a.href));
    if (byExtension) return byExtension.href;
    return null;
  }

  const adapter = { name: 'gamebanana', findInsertionPoint, findDirectDownloadUrl };
  DDMM.siteAdapters = DDMM.siteAdapters || {};
  DDMM.siteAdapters.gamebanana = adapter;

  if (DDMM.content) DDMM.content.runSiteAdapter(adapter);
})();
