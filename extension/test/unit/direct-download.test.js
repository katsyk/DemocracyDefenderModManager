import { beforeAll, beforeEach, describe, expect, it } from 'vitest';

// "Install with DDMM" on a page with a direct file link (GitHub,
// GameBanana, ModWorkshop, AyakaMods when logged in) starts the download
// itself and leaves the button on "Installing…" until the tab hears back.
// A download that never completes (cancelled, failed, refused) must still
// be reported, or the button stays stuck until the page is reloaded.

const listeners = { onMessage: [], onChanged: [] };
const evt = (name) => ({ addListener: (fn) => name && listeners[name].push(fn) });

/** What the fake `downloads.download` does next: an id, or an Error to reject with. */
let nextDownload = 1;
const tabMessages = [];
const installs = [];

beforeAll(async () => {
  globalThis.chrome = {
    runtime: {
      getManifest: () => ({ version: '1.0.0' }),
      connectNative: () => {
        const onMessage = [];
        return {
          postMessage(msg) {
            if (msg.type === 'install') installs.push(msg);
            const reply = { id: msg.id, ok: true, type: 'installed', mod: { name: 'Cool Mod' }, updated: false, deployed: true };
            queueMicrotask(() => onMessage.forEach((fn) => fn(reply)));
          },
          onMessage: { addListener: (fn) => onMessage.push(fn) },
          onDisconnect: { addListener() {} },
          disconnect() {},
        };
      },
      onMessage: evt('onMessage'),
      lastError: null,
    },
    storage: { local: { get: (_k, cb) => cb({}), set: (_v, cb) => cb && cb(), remove: (_k, cb) => cb && cb() } },
    downloads: {
      download(_opts, cb) {
        if (nextDownload instanceof Error) {
          globalThis.chrome.runtime.lastError = { message: nextDownload.message };
          cb(undefined);
          globalThis.chrome.runtime.lastError = null;
        } else {
          cb(nextDownload);
        }
      },
      search: (q, cb) => cb([{ id: q.id, state: 'complete', filename: '/dl/mod.zip', url: 'https://github.com/o/r/releases/download/v1/mod.zip' }]),
      onChanged: evt('onChanged'),
      onCreated: evt(),
    },
    tabs: { query: (_q, cb) => cb([]), sendMessage: (tabId, msg, cb) => { tabMessages.push({ tabId, msg }); if (cb) cb(); } },
    contextMenus: { create() {}, onClicked: evt() },
    notifications: { create: (_o, cb) => cb && cb() },
  };
  for (const lib of [
    'browser-shim.js', 'sources.js', 'errors.js', 'protocol-client.js',
    'capture.js', 'downloads-adapter.js', 'storage.js',
  ]) {
    await import(`../../src/lib/${lib}`);
  }
  await import('../../src/background/main.js');
});

beforeEach(() => {
  tabMessages.length = 0;
  installs.length = 0;
});

const flush = () => new Promise((r) => setTimeout(r, 20));
const PAGE = 'https://github.com/o/r/releases';
const FILE = 'https://github.com/o/r/releases/download/v1/mod.zip';

/** The content script's direct-link click, from tab 5. */
function clickInstall() {
  return new Promise((resolve) => {
    listeners.onMessage[0]({ type: 'ddmm:installDirect', url: FILE, pageUrl: PAGE, pageVersion: null }, { tab: { id: 5 } }, resolve);
  });
}

function changeState(id, current, previous = 'in_progress') {
  listeners.onChanged.forEach((fn) => fn({ id, state: { current, previous } }));
}

const resultsFor = (tabId) => tabMessages.filter((m) => m.tabId === tabId && m.msg.type === 'ddmm:installResult');

describe('direct-link installs whose download never completes', () => {
  it('tells the tab when the download is cancelled or fails', async () => {
    nextDownload = 41;
    expect(await clickInstall()).toMatchObject({ ok: true, downloadId: 41 });
    changeState(41, 'interrupted');
    await flush();
    const results = resultsFor(5);
    expect(results).toHaveLength(1);
    expect(results[0].msg.reply).toMatchObject({ ok: false, error: { code: 'DOWNLOAD_FAILED' } });
    expect(installs).toHaveLength(0);
  });

  it('still installs a download the user resumes after it was interrupted', async () => {
    nextDownload = 42;
    await clickInstall();
    changeState(42, 'interrupted');
    changeState(42, 'complete', 'interrupted');
    await flush();
    expect(installs).toHaveLength(1);
    expect(resultsFor(5).map((m) => m.msg.reply.ok)).toEqual([false, true]);
  });

  it('answers (and tells the tab) when the browser refuses to start the download', async () => {
    nextDownload = new Error('Download canceled by the user');
    const reply = await clickInstall();
    expect(reply).toMatchObject({ ok: false, error: { code: 'DOWNLOAD_FAILED' } });
    expect(resultsFor(5)).toHaveLength(1);
  });

  it('ignores interruptions of downloads it did not start', async () => {
    changeState(999, 'interrupted');
    await flush();
    expect(tabMessages).toHaveLength(0);
  });
});
