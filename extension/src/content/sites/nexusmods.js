/**
 * @file Nexus Mods content script.
 *
 * Every fetch of nexusmods.com in this task (multiple mod pages, multiple
 * user agents, both curl and a browser-like Accept/Accept-Language set)
 * returned Cloudflare's interactive challenge page ("Just a moment...", the
 * managed-challenge JS), never the real DOM -- see the task report for the
 * attempts and extension/test/fixtures/ for what came back. No selector
 * below is claimed to be verified against real Nexus markup; this adapter
 * always uses the floating-button fallback and the capture-arm download
 * path, which needs no site-specific selector to work correctly.
 *
 * This is also a deliberate policy, not just a DOM-access limitation: free
 * Nexus downloads require the user to click "Slow download" themselves
 * (Nexus's own rate-limiting/anti-bot mechanism), and the task spec is
 * explicit that DDMM must never automate that click. So even with perfect
 * DOM access, `findDirectDownloadUrl` would still deliberately return
 * `null` here -- the only difference real DOM access would make is a nicer
 * inline button placement instead of the floating corner button.
 */
/* global DDMM */
(function initNexusMods() {
  'use strict';

  const adapter = {
    name: 'nexus',
    // No verified insertion point -> floating fallback (see file header).
    findInsertionPoint: () => null,
    // Deliberately never returns a URL: Nexus free downloads must be a
    // manual, human click on Nexus's own "Slow download" button.
    findDirectDownloadUrl: () => null,
  };
  DDMM.siteAdapters = DDMM.siteAdapters || {};
  DDMM.siteAdapters.nexus = adapter;

  if (DDMM.content) DDMM.content.runSiteAdapter(adapter);
})();
