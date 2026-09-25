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

  /**
   * Chrome shuts an idle MV3 service worker down after ~30 s, and nothing
   * keeps it awake while the user waits on a download -- so the armed
   * capture (and recent unclaimed downloads) are mirrored into
   * `storage.session` and restored on the next wake-up. Memory-only where
   * `storage.session` doesn't exist.
   */
  const SESSION_KEY = 'ddmmCapture';
  const registryReady = (async () => {
    if (!api.storage.session) return;
    try {
      const data = await api.storage.session.get(SESSION_KEY);
      captureRegistry.restore(data && data[SESSION_KEY]);
    } catch {
      // Best effort: a fresh registry still works for this wake-up.
    }
  })();

  function persistRegistry() {
    if (!api.storage.session) return;
    api.storage.session.set({ [SESSION_KEY]: captureRegistry.serialize() }).catch(() => {
      // Best effort, see registryReady.
    });
  }

  /** downloadId -> context for downloads *we* triggered via `downloads.download()`. */
  const directDownloads = new Map();
  /** site -> timer that fires if an armed capture never matches. */
  const armTimers = new Map();
  /**
   * pageUrl -> the version its content script scraped, from the `ddmm:query`
   * each mod page sends on load. Lets capture installs send the version of
   * the page the download came from -- only ever when the file is that same
   * mod (see sendableVersion).
   */
  const pageVersions = new Map();
  const MAX_PAGE_VERSIONS = 200;

  /** @param {string|null} pageUrl @param {string|null|undefined} version */
  function rememberPageVersion(pageUrl, version) {
    if (!pageUrl || !version) return;
    pageVersions.delete(pageUrl);
    pageVersions.set(pageUrl, version);
    if (pageVersions.size > MAX_PAGE_VERSIONS) pageVersions.delete(pageVersions.keys().next().value);
  }

  /**
   * The page's scraped version, but only when the download is that page's
   * mod -- never another mod's version.
   * @param {{sameModAsPage: boolean}} attribution
   * @param {string|null} pageUrl
   * @param {string|null} [explicitVersion]
   */
  function sendableVersion(attribution, pageUrl, explicitVersion) {
    if (!attribution.sameModAsPage || !pageUrl) return null;
    return explicitVersion || pageVersions.get(pageUrl) || null;
  }

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
    directDownloads.set(downloadId, { ...context, downloadUrl: url });
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
        // The URL the user actually chose (the right-clicked link, or the
        // page's download link) -- not wherever it redirected to.
        downloadUrl: context.downloadUrl || normalized.finalUrl,
        pageVersion: context.pageVersion || null,
      });
      if (context.tabId != null) {
        sendToTab(context.tabId, { type: 'ddmm:installResult', reply });
      }
      return;
    }

    // Not something we initiated -- check armed (manual) capture, then
    // per-site auto-capture. However the download was started (Nexus's own
    // button, a userscript, a download manager, another tab), only where it
    // came from matters.
    await registryReady;
    const armed = captureRegistry.match(normalized);
    if (armed) {
      persistRegistry();
      clearArmTimer(armed.site);
      await installCaptured(normalized, armed);
      return;
    }

    for (const site of Object.keys(DDMM.capture.SITE_HOSTS)) {
      if (!DDMM.capture.isFromSite(normalized, site)) continue;
      if (!DDMM.capture.isArchiveDownload(normalized)) continue;
      // eslint-disable-next-line no-await-in-loop -- at most 5 sites, and short-circuits on the first host match
      if (await DDMM.storage.isAutoCaptureEnabled(site)) {
        const attribution = DDMM.sources.attributeDownload({
          contextPageUrl: normalized.referrer,
          downloadUrl: normalized.finalUrl,
          trustPage: true,
        });
        const reply = await runInstall({
          file: normalized.filename,
          pageUrl: attribution.pageUrl,
          downloadUrl: normalized.finalUrl,
          pageVersion: sendableVersion(attribution, normalized.referrer),
        });
        broadcastToSiteTabs(site, { type: 'ddmm:installResult', reply });
        return;
      }
      break;
    }

    // Nobody wanted it (yet). If the user clicks "Install with DDMM" on
    // this mod's page within a few minutes, it's picked up then.
    captureRegistry.remember(normalized);
    persistRegistry();
  }

  /**
   * Install a download claimed by an armed capture, and keep the tab that
   * armed it informed.
   * @param {object} normalized - A normalized download item.
   * @param {{site: string, pageUrl: string|null, pageVersion: string|null, tabId: number|null}} armed
   */
  async function installCaptured(normalized, armed) {
    const notifyTab = (message) =>
      armed.tabId != null ? sendToTab(armed.tabId, message) : broadcastToSiteTabs(armed.site, message);
    notifyTab({ type: 'ddmm:captureStarted', site: armed.site });
    const contextPageUrl = armed.pageUrl || normalized.referrer;
    const attribution = DDMM.sources.attributeDownload({
      contextPageUrl,
      downloadUrl: normalized.finalUrl,
      trustPage: true,
    });
    const reply = await runInstall({
      file: normalized.filename,
      pageUrl: attribution.pageUrl,
      downloadUrl: normalized.finalUrl,
      pageVersion: sendableVersion(attribution, contextPageUrl, armed.pageVersion),
    });
    notifyTab({ type: 'ddmm:installResult', reply });
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

  /**
   * What to install a right-clicked link as. The tab is only context: the
   * link may be a different mod than the page it's on (a "related mods"
   * link on mod A's page), so the tab URL is never the source unless the
   * link is that same mod. Otherwise `pageUrl` is the link itself when
   * it's a recognised mod URL, else null (the app then uses downloadUrl
   * host detection, which never assigns an id).
   * @param {{menuItemId: string, linkUrl?: string}} info
   * @param {{url?: string, id?: number}|undefined} tab
   * @returns {{pageUrl: string|null, pageVersion: string|null, tabId: number|null}|null}
   */
  function contextMenuInstallContext(info, tab) {
    if (info.menuItemId !== 'ddmm-install-link' || !info.linkUrl) return null;
    const tabUrl = (tab && tab.url) || null;
    const attribution = DDMM.sources.attributeDownload({
      contextPageUrl: tabUrl,
      downloadUrl: info.linkUrl,
      trustPage: false,
    });
    return {
      pageUrl: attribution.pageUrl,
      pageVersion: sendableVersion(attribution, tabUrl),
      tabId: tab && tab.id != null ? tab.id : null,
    };
  }

  if (api.contextMenus) {
    api.contextMenus.create({
      id: 'ddmm-install-link',
      title: 'Install with DDMM',
      contexts: ['link'],
    });

    api.contextMenus.onClicked.addListener((info, tab) => {
      const payload = contextMenuInstallContext(info, tab);
      if (!payload) return;
      startDirectDownload(info.linkUrl, payload).catch((e) =>
        notify('DDMM', `Couldn't start that download: ${e.message}`),
      );
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
        rememberPageVersion(message.pageUrl, message.pageVersion);
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
        const pageUrl = message.pageUrl ?? null;
        const pageVersion = message.pageVersion ?? null;
        await registryReady;
        clearArmTimer(message.site);

        // Already downloaded? (A userscript or download manager may have
        // fetched the file before the user got to this button.) Only a file
        // that provably is this page's mod is taken.
        const early = captureRegistry.claimRecent(message.site, pageUrl);
        if (early) {
          captureRegistry.disarm(message.site);
          persistRegistry();
          installCaptured(early, { site: message.site, pageUrl, pageVersion, tabId }).catch(() => {
            // Surfaced via notify()/installResult inside installCaptured.
          });
          return { ok: true, claimed: true };
        }

        captureRegistry.arm(message.site, { tabId, pageUrl, pageVersion });
        persistRegistry();
        armTimers.set(
          message.site,
          setTimeout(() => {
            armTimers.delete(message.site);
            if (tabId != null) {
              sendToTab(tabId, { type: 'ddmm:captureExpired', site: message.site });
            }
          }, DDMM.capture.DEFAULT_ARM_WINDOW_MS),
        );
        return { ok: true, claimed: false };
      }

      case 'ddmm:disarmCapture':
        await registryReady;
        captureRegistry.disarm(message.site);
        persistRegistry();
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

  DDMM.background = { handleMessage, runInstall, captureRegistry, directDownloads, contextMenuInstallContext };
})(typeof globalThis !== 'undefined' ? globalThis : this);
