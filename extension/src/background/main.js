/**
 * @file Background service worker (Chrome) / event page (Firefox) entry
 * point. Owns the single native-messaging connection to DDMM, the
 * `downloads` capture logic, the context menu, and message routing for
 * content scripts and the popup.
 *
 * Depends on (loaded first, see manifest `background.scripts` /
 * `importScripts` below): browser-shim.js, sources.js, errors.js,
 * protocol-client.js, capture.js, downloads-adapter.js, storage.js.
 */
/* global DDMM, importScripts */
(function initBackground(root) {
  'use strict';

  // Chrome's MV3 service worker is a classic script (no `type: module` is
  // declared for it), so it loads its dependencies with importScripts;
  // Firefox lists the same files directly in `background.scripts` (see
  // BACKGROUND_LIBS in scripts/manifest.mjs) and they share this global
  // scope already, making importScripts both unavailable and unnecessary
  // there. Don't key this off `browser` being defined: current Chrome
  // exposes a `browser` namespace in the service worker too, which left
  // DDMM undefined and the whole background script dead.
  if (typeof importScripts === 'function') {
    importScripts(
      '../lib/browser-shim.js',
      '../lib/sources.js',
      '../lib/errors.js',
      '../lib/protocol-client.js',
      '../lib/capture.js',
      '../lib/downloads-adapter.js',
      '../lib/storage.js',
    );
  }

  const api = DDMM.browserApi;
  const client = new DDMM.ProtocolClient();
  const captureRegistry = new DDMM.capture.CaptureRegistry();

  /** downloadId -> context for downloads *we* triggered via `downloads.download()`. */
  const directDownloads = new Map();
  /** site -> timer that fires if an armed capture never matches. */
  const armTimers = new Map();

  const EXTENSION_VERSION = api.runtime.getManifest().version;
  const BROWSER_NAME = DDMM.isFirefox ? 'firefox' : 'chrome';

  // ---------------------------------------------------------------------
  // Native messaging helpers
  // ---------------------------------------------------------------------

  /**
   * Turns a thrown client error (timeout, port disconnect, host missing)
   * into the same `{ok: false, error: {code, message}}` shape as an error
   * reply, so callers only handle one shape.
   * @param {unknown} e
   * @returns {{ok: false, error: {code: string, message: string}}}
   */
  function errorReply(e) {
    return { ok: false, error: { code: (e && e.code) || 'INTERNAL', message: (e && e.message) || '' } };
  }

  /**
   * Never starts DDMM (the host only launches it for `install`/`open`), so
   * it's safe to call just because a mod page loaded.
   * @returns {Promise<object>} The hello reply, or an error reply.
   */
  async function tryHello() {
    try {
      return await client.hello({ extensionVersion: EXTENSION_VERSION, browser: BROWSER_NAME });
    } catch (e) {
      return errorReply(e);
    }
  }

  /**
   * @param {{name: string, provider: string, version: string|null, updated: boolean}} mod
   */
  async function recordInstall(mod, updated) {
    await DDMM.storage.pushRecentInstall({
      name: mod && mod.name,
      provider: mod && mod.source && mod.source.provider,
      version: mod && mod.version,
      updated: Boolean(updated),
      at: Date.now(),
    });
  }

  /** @param {string} title @param {string} message */
  function notify(title, message) {
    if (!api.notifications) return;
    api.notifications
      .create({ type: 'basic', iconUrl: '../icons/icon128.png', title, message })
      .catch(() => {
        // Notifications are best-effort; a permission/platform quirk here
        // must never break the install flow itself.
      });
  }

  /**
   * Runs one `install` request end to end and reports the outcome back to
   * the originating tab (if any) via `captureResult`/direct reply.
   * @param {{file: string, pageUrl: string|null, downloadUrl: string|null, pageVersion?: string|null, afterInstall?: string|null}} params
   * @returns {Promise<object>} The reply message (success or `{ok:false, error}`).
   */
  async function runInstall(params) {
    const override = await DDMM.storage.getAfterInstallOverride();
    try {
      const reply = await client.install({
        ...params,
        afterInstall: params.afterInstall !== undefined ? params.afterInstall : override,
      });
      if (reply.ok) {
        await recordInstall(reply.mod, reply.updated);
        notify(
          'DDMM',
          `${reply.mod ? reply.mod.name : 'Mod'} installed${reply.deployed ? ' and deployed' : ''}.`,
        );
      } else {
        notify('DDMM install failed', DDMM.errors.describeError(reply.error && reply.error.code, reply.error && reply.error.message));
      }
      return reply;
    } catch (e) {
      const message = DDMM.errors.describeError(e && e.code, e && e.message);
      notify('DDMM install failed', message);
      return { ok: false, error: { code: (e && e.code) || 'INTERNAL', message } };
    }
  }

  // ---------------------------------------------------------------------
  // downloads.download() flows we initiated ourselves (direct link found,
  // or the link context menu): we already know the page/version context,
  // so we just wait for completion and install.
  // ---------------------------------------------------------------------

  /**
   * @param {string} url
   * @param {{pageUrl: string|null, pageVersion?: string|null, tabId: number|null}} context
   * @returns {Promise<number>} The new downloadId.
   */
  async function startDirectDownload(url, context) {
    const downloadId = await api.downloads.download({ url });
    directDownloads.set(downloadId, context);
    return downloadId;
  }

  api.downloads.onChanged.addListener((delta) => {
    const changed = DDMM.downloadsAdapter.normalizeChangedDelta(delta);
    if (changed.state !== 'complete') return;

    handleCompletedDownload(changed.id).catch(() => {
      // Errors are already surfaced via notify()/message replies inside
      // handleCompletedDownload; nothing more to do with a rejected promise
      // in an event listener.
    });
  });

  /** @param {number} downloadId */
  async function handleCompletedDownload(downloadId) {
    const [item] = await api.downloads.search({ id: downloadId });
    if (!item) return;
    const normalized = DDMM.downloadsAdapter.normalizeDownloadItem(item);

    if (directDownloads.has(downloadId)) {
      const context = directDownloads.get(downloadId);
      directDownloads.delete(downloadId);
      const reply = await runInstall({
        file: normalized.filename,
        pageUrl: context.pageUrl,
        downloadUrl: normalized.finalUrl,
        pageVersion: context.pageVersion || null,
      });
      if (context.tabId != null) {
        sendToTab(context.tabId, { type: 'ddmm:installResult', reply });
      }
      return;
    }

    // Not something we initiated -- check armed (manual) capture, then
    // per-site auto-capture.
    const armed = captureRegistry.match(normalized);
    if (armed) {
      clearArmTimer(armed.site);
      const reply = await runInstall({
        file: normalized.filename,
        pageUrl: armed.pageUrl || normalized.referrer,
        downloadUrl: normalized.finalUrl,
        pageVersion: null,
      });
      if (armed.tabId != null) {
        sendToTab(armed.tabId, { type: 'ddmm:installResult', reply });
      } else {
        broadcastToSiteTabs(armed.site, { type: 'ddmm:installResult', reply });
      }
      return;
    }

    for (const site of Object.keys(DDMM.capture.SITE_HOSTS)) {
      if (!DDMM.capture.isFromSite(normalized, site)) continue;
      if (!DDMM.capture.isArchiveDownload(normalized)) continue;
      // eslint-disable-next-line no-await-in-loop -- at most 5 sites, and short-circuits on the first host match
      if (await DDMM.storage.isAutoCaptureEnabled(site)) {
        const reply = await runInstall({
          file: normalized.filename,
          pageUrl: normalized.referrer,
          downloadUrl: normalized.finalUrl,
          pageVersion: null,
        });
        broadcastToSiteTabs(site, { type: 'ddmm:installResult', reply });
      }
      return;
    }
  }

  /** @param {string} site */
  function clearArmTimer(site) {
    const timer = armTimers.get(site);
    if (timer) {
      clearTimeout(timer);
      armTimers.delete(site);
    }
  }

  /**
   * @param {number} tabId
   * @param {object} message
   */
  function sendToTab(tabId, message) {
    api.tabs.sendMessage(tabId, message).catch(() => {
      // The tab may have navigated away or closed; nothing to update.
    });
  }

  /**
   * @param {string} site
   * @param {object} message
   */
  async function broadcastToSiteTabs(site, message) {
    const hosts = DDMM.capture.SITE_HOSTS[site] || [];
    const patterns = hosts.map((h) => `*://*.${h}/*`).concat(hosts.map((h) => `*://${h}/*`));
    try {
      const tabs = await api.tabs.query({ url: patterns });
      for (const tab of tabs) sendToTab(tab.id, message);
    } catch {
      // tabs.query with a `url` filter needs host permissions we may not
      // hold for a given site's CDN-only hosts; the arm-timeout path still
      // covers the user in that case.
    }
  }

  // ---------------------------------------------------------------------
  // Context menu: "Install with DDMM" on any link, any site.
  // ---------------------------------------------------------------------

  if (api.contextMenus) {
    api.contextMenus.create({
      id: 'ddmm-install-link',
      title: 'Install with DDMM',
      contexts: ['link'],
    });

    api.contextMenus.onClicked.addListener((info, tab) => {
      if (info.menuItemId !== 'ddmm-install-link' || !info.linkUrl) return;
      startDirectDownload(info.linkUrl, {
        pageUrl: tab ? tab.url : null,
        pageVersion: null,
        tabId: tab ? tab.id : null,
      }).catch((e) => notify('DDMM', `Couldn't start that download: ${e.message}`));
    });
  }

  // ---------------------------------------------------------------------
  // Message router: content scripts and the popup talk to the background
  // through this single entry point.
  // ---------------------------------------------------------------------

  /**
   * @param {object} message
   * @param {object} sender
   * @returns {Promise<object>}
   */
  async function handleMessage(message, sender) {
    switch (message && message.type) {
      case 'ddmm:hello':
        return tryHello();

      case 'ddmm:query': {
        try {
          return await client.query({ pageUrl: message.pageUrl, pageVersion: message.pageVersion ?? null });
        } catch (e) {
          return errorReply(e);
        }
      }

      case 'ddmm:status': {
        try {
          return await client.status();
        } catch (e) {
          return errorReply(e);
        }
      }

      // The popup's "Start DDMM" button: an explicit user action, so it may
      // launch DDMM (see bridge-protocol.md).
      case 'ddmm:open': {
        try {
          return await client.open();
        } catch (e) {
          return errorReply(e);
        }
      }

      case 'ddmm:installDirect': {
        const tabId = sender.tab ? sender.tab.id : null;
        const downloadId = await startDirectDownload(message.url, {
          pageUrl: message.pageUrl ?? null,
          pageVersion: message.pageVersion ?? null,
          tabId,
        });
        return { ok: true, downloadId };
      }

      case 'ddmm:armCapture': {
        const tabId = sender.tab ? sender.tab.id : null;
        captureRegistry.arm(message.site, { tabId, pageUrl: message.pageUrl ?? null });
        clearArmTimer(message.site);
        armTimers.set(
          message.site,
          setTimeout(() => {
            armTimers.delete(message.site);
            if (tabId != null) {
              sendToTab(tabId, { type: 'ddmm:captureExpired', site: message.site });
            }
          }, DDMM.capture.DEFAULT_ARM_WINDOW_MS),
        );
        return { ok: true };
      }

      case 'ddmm:disarmCapture':
        captureRegistry.disarm(message.site);
        clearArmTimer(message.site);
        return { ok: true };

      case 'ddmm:getAutoCapture':
        return { enabled: await DDMM.storage.isAutoCaptureEnabled(message.site) };

      case 'ddmm:getAllAutoCapture':
        return { sites: await DDMM.storage.getAllAutoCapture() };

      case 'ddmm:setAutoCapture':
        await DDMM.storage.setAutoCaptureEnabled(message.site, message.enabled);
        return { ok: true };

      case 'ddmm:getAfterInstallOverride':
        return { value: await DDMM.storage.getAfterInstallOverride() };

      case 'ddmm:setAfterInstallOverride':
        await DDMM.storage.setAfterInstallOverride(message.value);
        return { ok: true };

      case 'ddmm:getRecentInstalls':
        return { installs: await DDMM.storage.getRecentInstalls() };

      default:
        return { ok: false, error: { code: 'UNSUPPORTED', message: `Unknown message type: ${message && message.type}` } };
    }
  }

  api.runtime.onMessage.addListener((message, sender, sendResponse) => {
    handleMessage(message, sender).then(sendResponse);
    return true; // keep the message channel open for the async response
  });

  DDMM.background = { handleMessage, runInstall, captureRegistry, directDownloads };
})(typeof globalThis !== 'undefined' ? globalThis : this);
