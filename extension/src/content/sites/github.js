/**
 * @file GitHub content script.
 *
 * Real DOM verified against live fetches of a releases index and a single
 * release tag page (see extension/test/fixtures/github-releases.html).
 * GitHub always lazy-loads release assets through an `<include-fragment>`
 * (`/…/releases/expanded_assets/<tag>`) -- even a single tag page's initial
 * HTML has no `a[href*="/releases/download/"]` links, on either the list or
 * tag view. In a real browser those load in once the "Assets" section is
 * expanded (a user action DDMM never simulates).
 *
 * DDMM only looks for direct asset links at *click* time, by which point
 * the user has had a chance to expand Assets themselves; if none are found
 * yet, the extension falls back to arming capture with a hint to expand
 * Assets and click a file -- the right-click "Install with DDMM" context
 * menu on the asset link also works here without any of this, on any site.
 *
 * Only runs on `/releases…` pages (not every page under a repo), even
 * though `source_from_page_url` itself treats any `github.com/<owner>/<repo>`
 * page as that repo's mod source -- showing an install button on a repo's
 * issue tracker would be nonsensical.
 */
/* global DDMM */
(function initGitHub() {
  'use strict';

  /** @param {Document} doc @returns {Element|null} */
  function findAssetAnchor(doc) {
    return doc.querySelector('a[href*="/releases/download/"]');
  }

  /** @param {Document} doc @returns {Element|null} */
  function findInsertionPoint(doc) {
    const anchor = findAssetAnchor(doc);
    return anchor ? anchor.closest('li, details, div') || anchor.parentElement : null;
  }

  /** @param {Document} doc @returns {string|null} */
  function findDirectDownloadUrl(doc) {
    const anchor = findAssetAnchor(doc);
    return anchor ? anchor.href : null;
  }

  const adapter = { name: 'github', findInsertionPoint, findDirectDownloadUrl };
  DDMM.siteAdapters = DDMM.siteAdapters || {};
  DDMM.siteAdapters.github = adapter;

  // Only run on /releases… pages -- see file header. Guarded the same way
  // as the other site adapters so this file can be loaded in isolation for
  // unit tests.
  if (DDMM.content && location.pathname.includes('/releases')) {
    DDMM.content.runSiteAdapter(adapter);
  }
})();
