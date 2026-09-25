/**
 * @file TEST-ONLY content script for the Playwright smoke test's local
 * fixture mod page (served on http://localhost). Never included in a
 * release build -- only `scripts/build.mjs --test` adds the
 * `http://localhost/*` host permission and this file to the manifest (see
 * `includeLocalhost` in scripts/manifest.mjs).
 *
 * Reuses the real AyakaMods finder functions (same markup shape as
 * extension/test/fixtures/ayakamods-mod.html) so the smoke test exercises
 * the actual button/query/install pipeline end to end, just against a page
 * that isn't one of the five real mod-site hosts (so `sourceFromPageUrl`
 * would otherwise reject it -- see `DDMM.content.runTestAdapter`).
 */
/* global DDMM */
(function initTestFixture() {
  'use strict';

  if (!DDMM.siteAdapters || !DDMM.siteAdapters.ayakamods) return;

  DDMM.content.runTestAdapter({
    name: 'test-fixture',
    findInsertionPoint: DDMM.siteAdapters.ayakamods.findInsertionPoint,
    findDirectDownloadUrl: DDMM.siteAdapters.ayakamods.findDirectDownloadUrl,
    scrapeVersion: DDMM.siteAdapters.ayakamods.scrapeVersion,
  });
})();
