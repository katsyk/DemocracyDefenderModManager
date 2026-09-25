import { beforeAll, beforeEach, describe, expect, it } from 'vitest';

// End-to-end-ish test of the background capture pipeline against a fake
// `chrome`: downloads complete (via downloads.onChanged), content scripts
// arm capture (via runtime.onMessage), and a fake native host records
// every `install` it's sent. Covers downloads started by something other
// than Nexus's own countdown (a userscript, a download manager, another
// extension): they can arrive before the user clicks "Install with DDMM",
// from a CDN host with no referrer, or from a different tab.

const listeners = { onMessage: [], onChanged: [] };
const evt = (name) => ({ addListener: (fn) => name && listeners[name].push(fn) });

/** id -> DownloadItem the fake `downloads.search` returns. */
const downloads = new Map();
/** Every `install` request the fake native host received. */
const installs = [];
/** Every message sent to a tab. */
const tabMessages = [];
let sessionStore = {};

function fakePort() {
  const onMessage = [];
  return {
    postMessage(msg) {
      let reply;
      if (msg.type === 'install') {
        installs.push(msg);
        reply = { id: msg.id, ok: true, type: 'installed', mod: { name: 'Cool Mod', source: { provider: 'nexus', id: '123' } }, updated: false, deployed: true };
      } else {
        reply = { id: msg.id, ok: false, error: { code: 'APP_NOT_RUNNING', message: '' } };
      }
      queueMicrotask(() => onMessage.forEach((fn) => fn(reply)));
    },
    onMessage: { addListener: (fn) => onMessage.push(fn) },
    onDisconnect: { addListener() {} },
    disconnect() {},
  };
}

beforeAll(async () => {
  globalThis.chrome = {
    runtime: {
      getManifest: () => ({ version: '1.0.0' }),
      connectNative: fakePort,
      onMessage: evt('onMessage'),
      lastError: null,
    },
    storage: {
      local: { get: (_k, cb) => cb({}), set: (_v, cb) => cb && cb(), remove: (_k, cb) => cb && cb() },
      session: {
        get: (_k, cb) => cb({ ...sessionStore }),
        set: (v, cb) => { sessionStore = { ...sessionStore, ...v }; if (cb) cb(); },
      },
    },
    downloads: {
      download() {},
      search: (q, cb) => cb(downloads.has(q.id) ? [downloads.get(q.id)] : []),
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

let nextId = 1;

beforeEach(() => {
  installs.length = 0;
  tabMessages.length = 0;
  downloads.clear();
  for (const site of Object.keys(globalThis.DDMM.capture.SITE_HOSTS)) {
    globalThis.DDMM.background.captureRegistry.disarm(site);
  }
  globalThis.DDMM.background.captureRegistry.clearRecent?.();
});

const flush = () => new Promise((r) => setTimeout(r, 20));

/** Simulate a browser download finishing. */
async function completeDownload(item) {
  const id = nextId++;
  downloads.set(id, { id, state: 'complete', mime: '', referrer: '', ...item });
  listeners.onChanged.forEach((fn) => fn({ id, state: { current: 'complete', previous: 'in_progress' } }));
  await flush();
}

/** Simulate the content script's "Install with DDMM" click on a tab. */
async function arm(pageUrl, tabId = 11, pageVersion = null) {
  const reply = await new Promise((resolve) => {
    listeners.onMessage[0]({ type: 'ddmm:armCapture', site: 'nexus', pageUrl, pageVersion }, { tab: { id: tabId } }, resolve);
  });
  await flush();
  return reply;
}

const FILE_PAGE = 'https://www.nexusmods.com/helldivers2/mods/123?tab=files&file_id=456';
const MOD_PAGE = 'https://www.nexusmods.com/helldivers2/mods/123';
const CDN_URL = 'https://supporter-files.nexus-cdn.com/6119/123/Cool%20Mod-123-1-0-1700000000.zip?md5=abc&expires=1700003600&user_id=1';
const CF_URL = 'https://cf-files.nexusmods.com/cdn/6119/123/Cool Mod-123-1-0-1700000000.zip?md5=abc&expires=1700003600&user_id=1';

describe('Nexus downloads started by the user\'s own tools', () => {
  it('captures a CDN download with no referrer while armed', async () => {
    await arm(FILE_PAGE, 11, '1.0');
    await completeDownload({ url: CDN_URL, finalUrl: CDN_URL, filename: '/dl/Cool Mod-123-1-0-1700000000.zip' });
    expect(installs).toHaveLength(1);
    expect(installs[0].file).toBe('/dl/Cool Mod-123-1-0-1700000000.zip');
    expect(installs[0].pageUrl).toBe(FILE_PAGE);
    expect(installs[0].pageVersion).toBe('1.0');
    expect(tabMessages.some((m) => m.tabId === 11 && m.msg.type === 'ddmm:installResult')).toBe(true);
  });

  it('captures a download that started in a different tab opened by the Nexus tab', async () => {
    await arm(FILE_PAGE, 11);
    // A userscript's window.open(cdnUrl, '_blank', 'noreferrer'): new tab,
    // no referrer, only the CDN URL says where it came from.
    await completeDownload({ url: CF_URL, finalUrl: CF_URL, referrer: '', filename: '/dl/cool.zip' });
    expect(installs).toHaveLength(1);
    expect(installs[0].pageUrl).toBe(FILE_PAGE);
  });

  it('captures a download that finished before the user clicked Install with DDMM', async () => {
    // The userscript skipped the timer and the file finished downloading
    // before the button was even clicked.
    await completeDownload({ url: CDN_URL, finalUrl: CDN_URL, referrer: FILE_PAGE, filename: '/dl/early.zip' });
    expect(installs).toHaveLength(0); // not armed, auto-capture off: nothing yet
    await arm(FILE_PAGE, 11);
    expect(installs).toHaveLength(1);
    expect(installs[0].file).toBe('/dl/early.zip');
    expect(installs[0].pageUrl).toBe(FILE_PAGE);
  });

  it('never claims an earlier download of a different mod when arming', async () => {
    const otherMod = 'https://supporter-files.nexus-cdn.com/6119/999/Other-999-1-0-1700000000.zip?md5=x';
    await completeDownload({ url: otherMod, finalUrl: otherMod, filename: '/dl/other.zip' });
    await arm(FILE_PAGE, 11);
    expect(installs).toHaveLength(0);
  });

  it('attributes a CDN download of another mod to that mod, not the armed page', async () => {
    await arm(FILE_PAGE, 11, '1.0');
    const otherMod = 'https://supporter-files.nexus-cdn.com/6119/999/Other-999-1-0-1700000000.zip?md5=x';
    await completeDownload({ url: otherMod, finalUrl: otherMod, filename: '/dl/other.zip' });
    expect(installs).toHaveLength(1);
    expect(installs[0].pageUrl).toBe('https://www.nexusmods.com/helldivers2/mods/999');
    expect(installs[0].pageVersion).toBeNull();
  });

  it('tells the tab as soon as the download is caught, before the install finishes', async () => {
    await arm(MOD_PAGE, 12);
    await completeDownload({ url: CDN_URL, finalUrl: CDN_URL, filename: '/dl/a.zip' });
    const types = tabMessages.filter((m) => m.tabId === 12).map((m) => m.msg.type);
    expect(types).toEqual(['ddmm:captureStarted', 'ddmm:installResult']);
  });

  it('does not install a download that finished long before arming', async () => {
    const reg = globalThis.DDMM.background.captureRegistry;
    const realNow = reg._now;
    let now = 1_000_000;
    reg._now = () => now;
    try {
      await completeDownload({ url: CDN_URL, finalUrl: CDN_URL, filename: '/dl/stale.zip' });
      now += globalThis.DDMM.capture.RECENT_WINDOW_MS + 1;
      await arm(FILE_PAGE, 11);
      expect(installs).toHaveLength(0);
    } finally {
      reg._now = realNow;
    }
  });
});
