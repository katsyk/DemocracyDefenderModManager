/**
 * @file Native messaging client for the `io.github.katsyk.ddmm` bridge host.
 *
 * Implements the request/reply framing from docs/development/bridge-protocol.md:
 * every request carries a string `id`, every reply echoes it; requests
 * time out client-side (installs get 5 minutes, everything else 10 seconds,
 * per the spec); the port reconnects on disconnect; and installs are
 * serialized (one in flight at a time), matching the app's own
 * serialization so a burst of clicks can't race it.
 */
(function initProtocolClient(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  const HOST_NAME = 'io.github.katsyk.ddmm';
  const INSTALL_TIMEOUT_MS = 5 * 60 * 1000;
  const DEFAULT_TIMEOUT_MS = 10 * 1000;

  /**
   * @typedef {object} PendingRequest
   * @property {(value: unknown) => void} resolve
   * @property {(reason: Error) => void} reject
   * @property {ReturnType<typeof setTimeout>} timer
   */

  /**
   * A reconnecting client for one native-messaging port, with serialized
   * installs. The `connectFn` indirection exists purely for testability: unit
   * tests pass in a fake port factory instead of touching a real
   * `runtime.connectNative`.
   */
  class ProtocolClient {
    /**
     * @param {object} [opts]
     * @param {() => object} [opts.connectFn] - Returns a `Port`-like object
     *   (`postMessage`, `onMessage.addListener`, `onDisconnect.addListener`,
     *   `disconnect`). Defaults to `DDMM.browserApi.runtime.connectNative`.
     * @param {() => string} [opts.idFn] - Generates request ids. Defaults to
     *   an incrementing counter.
     */
    constructor(opts = {}) {
      this._connectFn =
        opts.connectFn ||
        (() => DDMM.browserApi.runtime.connectNative(HOST_NAME));
      this._idFn = opts.idFn || (() => String(++this._counter));
      this._counter = 0;
      /** @type {Map<string, PendingRequest>} */
      this._pending = new Map();
      /** @type {object|null} */
      this._port = null;
      /** Serializes install requests: resolves to "go ahead" one at a time. */
      this._installQueue = Promise.resolve();
    }

    /** @returns {object} The live port, connecting it first if needed. */
    _ensurePort() {
      if (this._port) return this._port;
      const port = this._connectFn();
      port.onMessage.addListener((msg) => this._handleMessage(msg));
      port.onDisconnect.addListener(() => this._handleDisconnect());
      this._port = port;
      return port;
    }

    /** @param {{id?: string}} msg */
    _handleMessage(msg) {
      if (!msg || typeof msg.id !== 'string') return;
      const pending = this._pending.get(msg.id);
      if (!pending) return;
      clearTimeout(pending.timer);
      this._pending.delete(msg.id);
      pending.resolve(msg);
    }

    _handleDisconnect() {
      this._port = null;
      const err = DDMM.browserApi.runtime.lastError;
      const reason = err && err.message ? err.message : 'DDMM disconnected';
      for (const [id, pending] of this._pending) {
        clearTimeout(pending.timer);
        pending.reject(Object.assign(new Error(reason), { code: 'DISCONNECTED' }));
        this._pending.delete(id);
      }
      // Reconnection happens lazily on the next send() call, per the "host
      // relay starts DDMM if needed" behavior in the protocol doc -- there's
      // no point holding a port open with nothing to say.
    }

    /**
     * Send one request and wait for its reply (or a timeout/disconnect).
     * @param {object} message - Everything except `id` (added here).
     * @param {number} [timeoutMs]
     * @returns {Promise<object>} The reply message.
     */
    send(message, timeoutMs = DEFAULT_TIMEOUT_MS) {
      const id = this._idFn();
      const port = this._ensurePort();
      const full = Object.assign({ id }, message);

      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          this._pending.delete(id);
          reject(Object.assign(new Error('Request timed out'), { code: 'TIMEOUT' }));
        }, timeoutMs);
        this._pending.set(id, { resolve, reject, timer });
        try {
          port.postMessage(full);
        } catch (e) {
          clearTimeout(timer);
          this._pending.delete(id);
          reject(e);
        }
      });
    }

    /**
     * `hello` -- checks whether DDMM is reachable and what version it is.
     * @param {{extensionVersion: string, browser: string}} info
     * @returns {Promise<object>}
     */
    hello(info) {
      return this.send({ type: 'hello', ...info }, DEFAULT_TIMEOUT_MS);
    }

    /**
     * `query` -- installed/update status for a mod page, for button labeling.
     * @param {{pageUrl: string, pageVersion?: string|null}} params
     * @returns {Promise<object>}
     */
    query(params) {
      return this.send({ type: 'query', ...params }, DEFAULT_TIMEOUT_MS);
    }

    /** `status` -- current app/game/profile status. @returns {Promise<object>} */
    status() {
      return this.send({ type: 'status' }, DEFAULT_TIMEOUT_MS);
    }

    /**
     * `install` -- serialized client-side so a burst of completed downloads
     * (auto-capture on a mod pack site, say) can't send overlapping installs;
     * they queue here and land one at a time, in order.
     * @param {object} params
     * @returns {Promise<object>}
     */
    install(params) {
      const run = () => this.send({ type: 'install', ...params }, INSTALL_TIMEOUT_MS);
      const result = this._installQueue.then(run, run);
      // Keep the queue alive regardless of this install's outcome, so one
      // failed/declined install doesn't wedge the ones behind it.
      this._installQueue = result.then(
        () => undefined,
        () => undefined,
      );
      return result;
    }

    /** Closes the port, if open. Mostly for tests and clean teardown. */
    disconnect() {
      if (this._port) {
        try {
          this._port.disconnect();
        } catch {
          // already gone
        }
      }
      this._port = null;
    }
  }

  DDMM.ProtocolClient = ProtocolClient;
  DDMM.protocolConstants = { HOST_NAME, INSTALL_TIMEOUT_MS, DEFAULT_TIMEOUT_MS };
})(typeof globalThis !== 'undefined' ? globalThis : this);
