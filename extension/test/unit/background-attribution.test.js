import { beforeAll, describe, expect, it } from 'vitest';

// Load the real background script against a minimal fake `chrome`, the
// same way Chrome's service worker would (minus importScripts: the
// libraries are imported here first, like Firefox's background.scripts).
const listeners = { onMessage: [] };
const evt = () => ({ addListener() {} });

beforeAll(async () => {
  globalThis.chrome = {
    runtime: {
      getManifest: () => ({ version: '1.0.0' }),
      connectNative: () => ({ postMessage() {}, onMessage: evt(), onDisconnect: evt(), disconnect() {} }),
      onMessage: { addListener: (fn) => listeners.onMessage.push(fn) },
      lastError: null,
    },
    storage: { local: { get: (_k, cb) => cb({}), set: (_v, cb) => cb && cb(), remove: (_k, cb) => cb && cb() } },
    downloads: { download() {}, search() {}, onChanged: evt(), onCreated: evt() },
    tabs: { query() {}, sendMessage() {} },
    contextMenus: { create() {}, onClicked: evt() },
    notifications: { create() {} },
  };
  for (const lib of [
    'browser-shim.js', 'sources.js', 'errors.js', 'protocol-client.js',
    'capture.js', 'downloads-adapter.js', 'storage.js',
  ]) {
    await import(`../../src/lib/${lib}`);
  }
  await import('../../src/background/main.js');
});

const PAGE_A = 'https://ayakamods.com/mods/mod-a.4084/';
const MENU = 'ddmm-install-link';

function payload(linkUrl, tabUrl = PAGE_A) {
  return globalThis.DDMM.background.contextMenuInstallContext(
    { menuItemId: MENU, linkUrl },
    { url: tabUrl, id: 7 },
  );
}

describe('context-menu install payload', () => {
  it("a link to mod B on mod A's page is never attributed to A", () => {
    const p = payload('https://ayakamods.com/mods/mod-b.5555/download');
    expect(p.pageUrl).toBe('https://ayakamods.com/mods/mod-b.5555/download');
    expect(p.pageUrl).not.toContain('4084');
    expect(p.pageVersion).toBeNull();
    expect(p.tabId).toBe(7);
  });

  it("a bare file link on mod A's page sends pageUrl: null (never the tab URL)", () => {
    const p = payload('https://cdn.ayakamods.com/attachments/whatever.zip');
    expect(p.pageUrl).toBeNull();
    expect(p.pageVersion).toBeNull();
  });

  it('a link on a non-mod page with no mod URL sends pageUrl: null', () => {
    expect(payload('https://cdn.example.org/files/x.zip', 'https://forum.example.org/t/1').pageUrl).toBeNull();
  });

  it("a link to the page's own mod keeps the page URL and may send its scraped version", async () => {
    // The page's content script reported its version via ddmm:query on load.
    for (const fn of listeners.onMessage) fn({ type: 'ddmm:query', pageUrl: PAGE_A, pageVersion: '1.2' }, {}, () => {});
    const p = payload('https://ayakamods.com/mods/mod-a.4084/download');
    expect(p.pageUrl).toBe(PAGE_A);
    expect(p.pageVersion).toBe('1.2');
  });

  it('ignores other menu items and links-less clicks', () => {
    const bg = globalThis.DDMM.background;
    expect(bg.contextMenuInstallContext({ menuItemId: 'other', linkUrl: 'https://x/y.zip' }, {})).toBeNull();
    expect(bg.contextMenuInstallContext({ menuItemId: MENU }, {})).toBeNull();
  });
});

describe('attributeDownload (capture paths)', () => {

  it('same mod as the page -> page URL, version allowed', () => {
    const a = globalThis.DDMM.sources.attributeDownload({
      contextPageUrl: PAGE_A, downloadUrl: 'https://ayakamods.com/mods/mod-a.4084/download', trustPage: true,
    });
    expect(a).toEqual({ pageUrl: PAGE_A, sameModAsPage: true });
  });

  it('a different mod than the page -> that mod, no version', () => {
    const a = globalThis.DDMM.sources.attributeDownload({
      contextPageUrl: PAGE_A, downloadUrl: 'https://ayakamods.com/mods/mod-b.5555/download', trustPage: true,
    });
    expect(a).toEqual({ pageUrl: 'https://ayakamods.com/mods/mod-b.5555/download', sameModAsPage: false });
  });

  it('an opaque CDN download keeps the trusted page but never claims the version', () => {
    const a = globalThis.DDMM.sources.attributeDownload({
      contextPageUrl: 'https://www.nexusmods.com/helldivers2/mods/123',
      downloadUrl: 'https://supporter-files.nexus-cdn.com/x/123/file.zip',
      trustPage: true,
    });
    expect(a).toEqual({ pageUrl: 'https://www.nexusmods.com/helldivers2/mods/123', sameModAsPage: false });
  });
});
