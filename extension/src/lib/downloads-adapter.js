/**
 * @file Normalizes `downloads.DownloadItem` differences between Chromium
 * and Firefox into one shape the rest of the extension uses.
 *
 * Known differences:
 *   - `finalUrl` (the URL after redirects) exists on Chromium's DownloadItem
 *     (since Chrome 54) but not Firefox's -- Firefox only has `url`, and
 *     redirects there still land in `url`. We treat `finalUrl || url` as
 *     "the real download URL" everywhere.
 *   - `filename` is an absolute, OS-native path on both browsers for a
 *     download that has started (Firefox has done this since 57), so no
 *     translation is needed there -- this adapter still centralizes the
 *     read so a future difference only needs one fix.
 */
(function initDownloadsAdapter(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  /**
   * @typedef {object} NormalizedDownload
   * @property {number} id
   * @property {string} state - `"in_progress"`, `"complete"`, `"interrupted"`.
   * @property {string|null} filename - Absolute path, once known.
   * @property {string|null} url - Original request URL.
   * @property {string|null} finalUrl - URL after redirects (falls back to `url` on Firefox).
   * @property {string|null} referrer
   * @property {string|null} mime
   */

  /**
   * @param {object} item - A raw `downloads.DownloadItem`.
   * @returns {NormalizedDownload}
   */
  function normalizeDownloadItem(item) {
    return {
      id: item.id,
      state: item.state,
      filename: item.filename || null,
      url: item.url || null,
      finalUrl: item.finalUrl || item.url || null,
      referrer: item.referrer || null,
      mime: item.mime || null,
    };
  }

  /**
   * `downloads.onChanged` delivers a *delta* (`{id, state: {current, previous}, ...}`),
   * not a full DownloadItem. This normalizes that shape too, reading only
   * the fields we need and their `.current` value where the API wraps one.
   * @param {object} delta - A raw `downloads.onChanged` delta.
   * @returns {{id: number, state: string|null, filenameChanged: boolean}}
   */
  function normalizeChangedDelta(delta) {
    return {
      id: delta.id,
      state: delta.state ? delta.state.current : null,
      filenameChanged: Boolean(delta.filename),
    };
  }

  DDMM.downloadsAdapter = { normalizeDownloadItem, normalizeChangedDelta };
})(typeof globalThis !== 'undefined' ? globalThis : this);
