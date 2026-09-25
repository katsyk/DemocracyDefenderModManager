/**
 * @file Builds the per-browser MV3 manifest object from one shared base.
 * Pure and side-effect-free so both `build.mjs` and the vitest suite can
 * import it directly.
 */

/** Known mod-site host permission / content-script match patterns. */
export const SITE_MATCHES = [
  '*://*.ayakamods.com/*',
  '*://*.nexusmods.com/*',
  '*://*.modworkshop.net/*',
  '*://*.gamebanana.com/*',
  '*://github.com/*',
];

/**
 * Library scripts the background script depends on, in load order, as
 * paths relative to the extension root. Chrome's service worker pulls them
 * in itself with `importScripts` (see background/main.js); Firefox's MV3
 * event page is a plain page with no `importScripts`, so they have to be
 * listed in `background.scripts` ahead of main.js instead.
 */
export const BACKGROUND_LIBS = [
  'lib/browser-shim.js',
  'lib/sources.js',
  'lib/errors.js',
  'lib/protocol-client.js',
  'lib/capture.js',
  'lib/downloads-adapter.js',
  'lib/storage.js',
];

/** Manifest fields identical on every browser. */
function baseManifest(version) {
  return {
    manifest_version: 3,
    name: 'DDMM: One-Click Mod Install',
    short_description: 'Install Helldivers 2 mods from any site with one click. Your mods. Your democracy. Defended.',
    description:
      "Adds an Install with DDMM button to AyakaMods, Nexus Mods, ModWorkshop, GameBanana and GitHub mod pages, so one click (or a right-click on any download link) hands the file straight to Democracy Defender Mod Manager. Every mod site is treated equally -- no site is second-class. Nothing leaves your computer except the downloads you ask for.",
    version,
    icons: {
      16: 'icons/icon16.png',
      32: 'icons/icon32.png',
      48: 'icons/icon48.png',
      128: 'icons/icon128.png',
    },
    action: {
      default_title: 'DDMM',
      default_popup: 'popup/popup.html',
      default_icon: {
        16: 'icons/icon16.png',
        32: 'icons/icon32.png',
        48: 'icons/icon48.png',
        128: 'icons/icon128.png',
      },
    },
    permissions: ['nativeMessaging', 'downloads', 'contextMenus', 'storage', 'notifications'],
    host_permissions: SITE_MATCHES,
    content_scripts: [
      {
        matches: ['*://*.ayakamods.com/mods/*'],
        js: [
          'lib/browser-shim.js',
          'lib/sources.js',
          'lib/errors.js',
          'lib/button-state.js',
          'lib/capture.js',
          'content/common.js',
          'content/sites/ayakamods.js',
        ],
        run_at: 'document_idle',
      },
      {
        matches: ['*://*.nexusmods.com/helldivers2/mods/*'],
        js: [
          'lib/browser-shim.js',
          'lib/sources.js',
          'lib/errors.js',
          'lib/button-state.js',
          'lib/capture.js',
          'content/common.js',
          'content/sites/nexusmods.js',
        ],
        run_at: 'document_idle',
      },
      {
        matches: ['*://*.modworkshop.net/mod/*'],
        js: [
          'lib/browser-shim.js',
          'lib/sources.js',
          'lib/errors.js',
          'lib/button-state.js',
          'lib/capture.js',
          'content/common.js',
          'content/sites/modworkshop.js',
        ],
        run_at: 'document_idle',
      },
      {
        matches: ['*://*.gamebanana.com/mods/*'],
        js: [
          'lib/browser-shim.js',
          'lib/sources.js',
          'lib/errors.js',
          'lib/button-state.js',
          'lib/capture.js',
          'content/common.js',
          'content/sites/gamebanana.js',
        ],
        run_at: 'document_idle',
      },
      {
        matches: ['*://github.com/*/*/releases*'],
        js: [
          'lib/browser-shim.js',
          'lib/sources.js',
          'lib/errors.js',
          'lib/button-state.js',
          'lib/capture.js',
          'content/common.js',
          'content/sites/github.js',
        ],
        run_at: 'document_idle',
      },
    ],
  };
}

/**
 * Test-only host permission for the local fixture server the Playwright
 * smoke test serves a mod page from. Never present in a release build --
 * only added when `includeLocalhost` is explicitly passed true, which
 * scripts/build.mjs only does for its `--test` mode.
 */
const LOCALHOST_TEST_MATCH = 'http://localhost/*';

/**
 * @param {{version: string, chromeKey: string, includeLocalhost?: boolean}} opts
 * @returns {object} The Chrome/Edge/Brave manifest.
 */
export function buildChromeManifest({ version, chromeKey, includeLocalhost = false }) {
  const manifest = baseManifest(version);
  manifest.key = chromeKey;
  manifest.background = {
    service_worker: 'background/main.js',
  };
  manifest.minimum_chrome_version = '116';
  if (includeLocalhost) {
    manifest.host_permissions = [...manifest.host_permissions, LOCALHOST_TEST_MATCH];
    manifest.content_scripts.push({
      matches: ['http://localhost/*'],
      js: [
        'lib/browser-shim.js',
        'lib/sources.js',
        'lib/errors.js',
        'lib/button-state.js',
        'lib/capture.js',
        'content/common.js',
        // ayakamods.js's own runSiteAdapter call is a no-op on localhost
        // (sourceFromPageUrl rejects the host); it's loaded here only to
        // populate DDMM.siteAdapters.ayakamods, which test-fixture.js reuses.
        'content/sites/ayakamods.js',
        'content/sites/test-fixture.js',
      ],
      run_at: 'document_idle',
    });
  }
  return manifest;
}

/**
 * @param {{version: string, geckoId: string, minFirefoxVersion: string}} opts
 * @returns {object} The Firefox manifest.
 */
export function buildFirefoxManifest({ version, geckoId, minFirefoxVersion }) {
  const manifest = baseManifest(version);
  // Firefox has no `background.service_worker` / manifest `key`; MV3 event
  // pages there are plain `background.scripts`, loaded as classic scripts
  // sharing one global scope, which is exactly the loading model the rest
  // of this codebase already uses (see extension/README.md).
  manifest.background = {
    scripts: [...BACKGROUND_LIBS, 'background/main.js'],
  };
  manifest.browser_specific_settings = {
    gecko: {
      id: geckoId,
      // MV3 `background.scripts` as a non-persistent event page needs
      // Firefox 109+; pinned a little higher for broader `chrome.*`-shim
      // and downloads-API parity headroom.
      strict_min_version: minFirefoxVersion,
      // Matches extension/PRIVACY.md: DDMM's extension collects nothing.
      // Newer Firefox requires this disclosure explicitly (see web-ext
      // lint's MISSING_DATA_COLLECTION_PERMISSIONS notice).
      data_collection_permissions: { required: ['none'] },
    },
  };
  return manifest;
}

/**
 * Derives the unpacked Chrome extension id from the manifest `key` (base64
 * DER SubjectPublicKeyInfo), the same way Chromium does: SHA-256 the DER
 * bytes, take the first 32 hex characters, and map each hex digit
 * 0-9/a-f -> a-p.
 * @param {string} base64Key
 * @returns {Promise<string>}
 */
export async function extensionIdFromKey(base64Key) {
  const { createHash } = await import('node:crypto');
  const der = Buffer.from(base64Key, 'base64');
  const hash = createHash('sha256').update(der).digest('hex');
  const first32 = hash.slice(0, 32);
  return first32.replace(/./g, (c) => String.fromCharCode(97 + parseInt(c, 16)));
}
