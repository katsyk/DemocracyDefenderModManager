/**
 * @file Cross-browser extension API shim.
 *
 * Chrome/Edge/Brave expose the extension API as `chrome.*` with a mix of
 * callback-based and (recently) promise-based signatures. Firefox exposes
 * `browser.*` with promise-based signatures throughout. This shim gives the
 * rest of the codebase a single, always-promise-based `DDMM.browserApi` to
 * call, so the same source files run unmodified on both browsers.
 *
 * Classic script: attaches to the shared `self.DDMM` namespace instead of
 * using ES module import/export. Content scripts in MV3 are always loaded
 * as classic scripts by both browsers (there is no manifest-level "module"
 * declaration for `content_scripts`), and Firefox's MV3 background pages
 * only reliably support classic `background.scripts`. Using plain classic
 * scripts with a shared namespace everywhere (background, content, popup)
 * keeps one code style, avoids `web_accessible_resources` just to support
 * `import()` from an isolated world, and needs no bundler. See
 * extension/README.md for the full rationale.
 */
/* global globalThis */
(function initBrowserShim(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  /** The native, un-shimmed extension API root (`browser` or `chrome`). */
  const raw = root.browser || root.chrome;

  /**
   * @param {Function} fn - A callback-style API method (`fn(...args, cb)`).
   * @param {object} thisArg - The object `fn` should be invoked on.
   * @returns {(...args: unknown[]) => Promise<unknown>} A promise-based wrapper.
   */
  function promisify(fn, thisArg) {
    return function promisified(...args) {
      // Firefox's `browser.*` already returns promises when no callback is
      // supplied. Detect that by trying the no-callback call first when the
      // native API root already exposes `browser` (Firefox, and modern
      // Chromium partially); fall back to callback-style otherwise.
      if (root.browser) {
        return fn.apply(thisArg, args);
      }
      return new Promise((resolve, reject) => {
        fn.call(thisArg, ...args, (result) => {
          const err = raw.runtime && raw.runtime.lastError;
          if (err) {
            reject(new Error(err.message || String(err)));
          } else {
            resolve(result);
          }
        });
      });
    };
  }

  /**
   * A minimal promise-based subset of the WebExtension API, covering only
   * what this extension uses. Namespaced objects are passed through as-is
   * where every member is already an object/primitive (e.g. `onChanged`
   * event emitters), and wrapped where a call needs promisification.
   */
  const browserApi = {
    runtime: raw.runtime,
    // downloads, contextMenus, notifications and action are all
    // background/extension-page-only APIs -- undefined in a content
    // script's `chrome`/`browser` object, even with the permission granted.
    // Every property below is guarded the same way so loading this shim in
    // a content script never throws; content scripts only ever use `runtime`
    // (and `storage`, which *is* available there).
    downloads: raw.downloads
      ? {
          download: promisify(raw.downloads.download, raw.downloads),
          search: promisify(raw.downloads.search, raw.downloads),
          onChanged: raw.downloads.onChanged,
          onCreated: raw.downloads.onCreated,
        }
      : undefined,
    storage: {
      local: {
        get: promisify(raw.storage.local.get, raw.storage.local),
        set: promisify(raw.storage.local.set, raw.storage.local),
        remove: promisify(raw.storage.local.remove, raw.storage.local),
      },
    },
    tabs: raw.tabs
      ? {
          query: promisify(raw.tabs.query, raw.tabs),
          sendMessage: promisify(raw.tabs.sendMessage, raw.tabs),
        }
      : undefined,
    contextMenus: raw.contextMenus,
    notifications: raw.notifications
      ? {
          create: promisify(raw.notifications.create, raw.notifications),
        }
      : undefined,
    action: raw.action || raw.browserAction,
  };

  DDMM.browserApi = browserApi;
  DDMM.isFirefox = Boolean(root.browser) && !root.chrome;
})(typeof globalThis !== 'undefined' ? globalThis : this);
