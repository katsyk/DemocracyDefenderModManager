import { beforeEach, describe, expect, it, vi } from 'vitest';

globalThis.DDMM = globalThis.DDMM || {};
globalThis.DDMM.browserApi = { runtime: { lastError: null } };

import '../../src/lib/protocol-client.js';

const { ProtocolClient } = globalThis.DDMM;

/** A minimal fake `runtime.Port`. */
class FakePort {
  constructor() {
    this.sent = [];
    this._messageListeners = [];
    this._disconnectListeners = [];
    this.disconnected = false;
    this.onMessage = { addListener: (fn) => this._messageListeners.push(fn) };
    this.onDisconnect = { addListener: (fn) => this._disconnectListeners.push(fn) };
  }
  postMessage(msg) {
    if (this.disconnected) throw new Error('port disconnected');
    this.sent.push(msg);
  }
  disconnect() {
    this.disconnected = true;
  }
  /** Test helper: simulate the native host replying. */
  reply(msg) {
    for (const fn of this._messageListeners) fn(msg);
  }
  /** Test helper: simulate the host process going away. */
  simulateDisconnect() {
    this.disconnected = true;
    for (const fn of this._disconnectListeners) fn();
  }
}

describe('ProtocolClient', () => {
  let port;
  let ids;
  let client;

  beforeEach(() => {
    port = new FakePort();
    ids = 0;
    client = new ProtocolClient({ connectFn: () => port, idFn: () => String(++ids) });
  });

  it('sends a request with a generated id and resolves on the matching reply', async () => {
    const promise = client.hello({ extensionVersion: '1.0.0', browser: 'chrome' });
    expect(port.sent).toEqual([{ id: '1', type: 'hello', extensionVersion: '1.0.0', browser: 'chrome' }]);
    port.reply({ id: '1', ok: true, type: 'hello', appVersion: '2.0.0-rc.4', protocol: 1 });
    await expect(promise).resolves.toMatchObject({ ok: true, appVersion: '2.0.0-rc.4' });
  });

  it('ignores replies for unknown ids', async () => {
    const promise = client.status();
    port.reply({ id: 'not-mine', ok: true });
    port.reply({ id: '1', ok: true, type: 'status', gameFound: true });
    await expect(promise).resolves.toMatchObject({ gameFound: true });
  });

  it('reuses the same port across requests', async () => {
    const p1 = client.hello({ extensionVersion: '1', browser: 'chrome' });
    port.reply({ id: '1', ok: true });
    await p1;
    const p2 = client.status();
    port.reply({ id: '2', ok: true });
    await p2;
    expect(port.sent.length).toBe(2);
  });

  it('times out a request that never gets a reply', async () => {
    vi.useFakeTimers();
    try {
      const promise = client.send({ type: 'query', pageUrl: 'x' }, 50);
      vi.advanceTimersByTime(51);
      await expect(promise).rejects.toMatchObject({ code: 'TIMEOUT' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('uses the long install timeout, not the default', async () => {
    vi.useFakeTimers();
    try {
      const promise = client.install({ file: 'x', pageUrl: null, downloadUrl: null });
      await Promise.resolve(); // let the queued install actually call send()
      vi.advanceTimersByTime(9 * 1000); // well past the 10s default, still under 5 minutes
      expect(vi.getTimerCount()).toBeGreaterThan(0);
      port.reply({ id: '1', ok: true, type: 'installed' });
      await promise;
    } finally {
      vi.useRealTimers();
    }
  });

  it('rejects in-flight requests on disconnect', async () => {
    const promise = client.status();
    port.simulateDisconnect();
    await expect(promise).rejects.toMatchObject({ code: 'DISCONNECTED' });
  });

  it('reports a missing native host as NATIVE_HOST_MISSING, not a plain disconnect', async () => {
    for (const message of [
      'Specified native messaging host not found.',
      'Access to the specified native messaging host is forbidden.',
      'No such native application io.github.katsyk.ddmm',
    ]) {
      port = new FakePort();
      client = new ProtocolClient({ connectFn: () => port, idFn: () => String(++ids) });
      const promise = client.hello({ extensionVersion: '1', browser: 'chrome' });
      globalThis.DDMM.browserApi.runtime.lastError = { message };
      port.simulateDisconnect();
      globalThis.DDMM.browserApi.runtime.lastError = null;
      await expect(promise).rejects.toMatchObject({ code: 'NATIVE_HOST_MISSING' });
    }
  });

  it('keeps DISCONNECTED for a host that crashed or exited', async () => {
    const promise = client.status();
    globalThis.DDMM.browserApi.runtime.lastError = { message: 'Native host has exited.' };
    port.simulateDisconnect();
    globalThis.DDMM.browserApi.runtime.lastError = null;
    await expect(promise).rejects.toMatchObject({ code: 'DISCONNECTED' });
  });

  it('open waits up to 60 s (it may be starting DDMM), longer than hello', async () => {
    vi.useFakeTimers();
    try {
      const { OPEN_TIMEOUT_MS, DEFAULT_TIMEOUT_MS } = globalThis.DDMM.protocolConstants;
      expect(OPEN_TIMEOUT_MS).toBeGreaterThanOrEqual(60_000);
      expect(DEFAULT_TIMEOUT_MS).toBeLessThan(OPEN_TIMEOUT_MS);

      let settled = false;
      const promise = client.open().then(
        (v) => { settled = true; return v; },
        (e) => { settled = true; throw e; },
      );
      expect(port.sent).toEqual([{ id: '1', type: 'open' }]);
      vi.advanceTimersByTime(45_000);
      await Promise.resolve();
      expect(settled).toBe(false);
      port.reply({ id: '1', ok: true, type: 'opened' });
      await expect(promise).resolves.toMatchObject({ type: 'opened' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('reconnects lazily on the next send after a disconnect', async () => {
    const ports = [port, new FakePort()];
    let call = 0;
    client = new ProtocolClient({ connectFn: () => ports[call++], idFn: () => String(++ids) });

    const p1 = client.status();
    ports[0].reply({ id: '1', ok: true });
    await p1;

    ports[0].simulateDisconnect();

    const p2 = client.status();
    ports[1].reply({ id: '2', ok: true });
    await p2;
    expect(ports[1].sent.length).toBe(1);
  });

  it('serializes installs: the second waits for the first to settle', async () => {
    const order = [];
    const p1 = client.install({ file: 'a', pageUrl: null, downloadUrl: null }).then(() => order.push('a'));
    const p2 = client.install({ file: 'b', pageUrl: null, downloadUrl: null }).then(() => order.push('b'));

    // Installs are queued via a promise chain (see ProtocolClient#install),
    // so give the first link's `.then` a turn to run before inspecting what
    // was actually sent on the port.
    await Promise.resolve();
    // Only the first install should have been sent so far.
    expect(port.sent.map((m) => m.file)).toEqual(['a']);

    port.reply({ id: '1', ok: true });
    await p1;
    // The queue's internal advance takes one more microtask tick than `p1`
    // itself; a real macrotask flush guarantees everything queued has run.
    await new Promise((r) => setTimeout(r, 0));
    // Now the second install's request goes out.
    expect(port.sent.map((m) => m.file)).toEqual(['a', 'b']);
    port.reply({ id: '2', ok: true });
    await p2;

    expect(order).toEqual(['a', 'b']);
  });

  it('a failed install does not block the next one', async () => {
    const p1 = client.install({ file: 'a', pageUrl: null, downloadUrl: null }).catch((e) => e);
    await Promise.resolve();
    port.reply({ id: '1', ok: false, error: { code: 'DECLINED', message: 'no' } });
    await p1;
    await new Promise((r) => setTimeout(r, 0));

    const p2 = client.install({ file: 'b', pageUrl: null, downloadUrl: null });
    await new Promise((r) => setTimeout(r, 0));
    expect(port.sent.map((m) => m.file)).toEqual(['a', 'b']);
    port.reply({ id: '2', ok: true });
    await expect(p2).resolves.toMatchObject({ ok: true });
  });
});
