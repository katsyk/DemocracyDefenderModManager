/**
 * @file `storage.local` schema and small helpers. Everything DDMM's
 * extension remembers lives under these keys; nothing here ever leaves the
 * browser (see extension/PRIVACY.md).
 */
(function initStorage(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  const KEYS = {
    AUTO_CAPTURE: 'autoCapture', // { [site: string]: boolean } -- off by default for every site
    AFTER_INSTALL: 'afterInstallOverride', // null | 'library' | 'profile' | 'deploy'
    RECENT_INSTALLS: 'recentInstalls', // Array<RecentInstall>, newest first, capped at 10
  };

  const MAX_RECENT_INSTALLS = 10;

  /**
   * @typedef {object} RecentInstall
   * @property {string} name
   * @property {string} provider
   * @property {string|null} version
   * @property {boolean} updated
   * @property {number} at - `Date.now()` at install time.
   */

  /**
   * @param {string} site
   * @returns {Promise<boolean>}
   */
  async function isAutoCaptureEnabled(site) {
    const data = await DDMM.browserApi.storage.local.get(KEYS.AUTO_CAPTURE);
    const map = data[KEYS.AUTO_CAPTURE] || {};
    return Boolean(map[site]);
  }

  /**
   * @param {string} site
   * @param {boolean} enabled
   * @returns {Promise<void>}
   */
  async function setAutoCaptureEnabled(site, enabled) {
    const data = await DDMM.browserApi.storage.local.get(KEYS.AUTO_CAPTURE);
    const map = data[KEYS.AUTO_CAPTURE] || {};
    map[site] = enabled;
    await DDMM.browserApi.storage.local.set({ [KEYS.AUTO_CAPTURE]: map });
  }

  /** @returns {Promise<Record<string, boolean>>} */
  async function getAllAutoCapture() {
    const data = await DDMM.browserApi.storage.local.get(KEYS.AUTO_CAPTURE);
    return data[KEYS.AUTO_CAPTURE] || {};
  }

  /** @returns {Promise<string|null>} */
  async function getAfterInstallOverride() {
    const data = await DDMM.browserApi.storage.local.get(KEYS.AFTER_INSTALL);
    return data[KEYS.AFTER_INSTALL] ?? null;
  }

  /** @param {string|null} value */
  async function setAfterInstallOverride(value) {
    await DDMM.browserApi.storage.local.set({ [KEYS.AFTER_INSTALL]: value });
  }

  /** @returns {Promise<RecentInstall[]>} */
  async function getRecentInstalls() {
    const data = await DDMM.browserApi.storage.local.get(KEYS.RECENT_INSTALLS);
    return data[KEYS.RECENT_INSTALLS] || [];
  }

  /** @param {RecentInstall} entry */
  async function pushRecentInstall(entry) {
    const existing = await getRecentInstalls();
    const next = [entry, ...existing].slice(0, MAX_RECENT_INSTALLS);
    await DDMM.browserApi.storage.local.set({ [KEYS.RECENT_INSTALLS]: next });
  }

  DDMM.storage = {
    KEYS,
    MAX_RECENT_INSTALLS,
    isAutoCaptureEnabled,
    setAutoCaptureEnabled,
    getAllAutoCapture,
    getAfterInstallOverride,
    setAfterInstallOverride,
    getRecentInstalls,
    pushRecentInstall,
  };
})(typeof globalThis !== 'undefined' ? globalThis : this);
