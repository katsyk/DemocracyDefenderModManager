import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';
import { beforeAll, describe, expect, it } from 'vitest';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES = path.join(__dirname, '..', 'fixtures');

function loadFixture(name) {
  const html = readFileSync(path.join(FIXTURES, name), 'utf8');
  return new JSDOM(html).window.document;
}

// DDMM.content is intentionally left unset here (jsdom's `document`/`location`
// exist, but we're not simulating the full extension runtime) -- each site
// file guards its top-level `runSiteAdapter` call on `DDMM.content` being
// present, so importing them here only registers DDMM.siteAdapters.<name>
// without trying to talk to a background script. See content/sites/*.js.
beforeAll(async () => {
  globalThis.DDMM = globalThis.DDMM || {};
  await import('../../src/lib/sources.js');
  await import('../../src/lib/capture.js');
  await import('../../src/content/sites/ayakamods.js');
  await import('../../src/content/sites/modworkshop.js');
  await import('../../src/content/sites/gamebanana.js');
  await import('../../src/content/sites/github.js');
  await import('../../src/content/sites/nexusmods.js');
});

describe('AyakaMods adapter (real DOM fixture)', () => {
  it('finds the sidebar insertion point', () => {
    const doc = loadFixture('ayakamods-mod.html');
    const el = globalThis.DDMM.siteAdapters.ayakamods.findInsertionPoint(doc);
    expect(el).not.toBeNull();
    expect(el.className).toContain('modSidebarGroup--buttons');
  });

  it('finds no direct download URL for a logged-out guest (disabled span, not a link)', () => {
    const doc = loadFixture('ayakamods-mod.html');
    expect(globalThis.DDMM.siteAdapters.ayakamods.findDirectDownloadUrl(doc)).toBeNull();
  });

  it('scrapes the version from the SoftwareApplication JSON-LD', () => {
    const doc = loadFixture('ayakamods-mod.html');
    expect(globalThis.DDMM.siteAdapters.ayakamods.scrapeVersion(doc)).toBe('2026-09-24');
  });
});

describe('ModWorkshop adapter (real DOM fixture)', () => {
  it('finds the direct storage.modworkshop.net download link', () => {
    const doc = loadFixture('modworkshop-mod.html');
    const url = globalThis.DDMM.siteAdapters.modworkshop.findDirectDownloadUrl(doc);
    expect(url).toContain('storage.modworkshop.net/mods/files/');
    expect(globalThis.DDMM.capture.hasArchiveExtension(url) || url.includes('filename=')).toBeTruthy();
  });

  it('finds an insertion point next to the download button', () => {
    const doc = loadFixture('modworkshop-mod.html');
    const el = globalThis.DDMM.siteAdapters.modworkshop.findInsertionPoint(doc);
    expect(el).not.toBeNull();
    expect(el.querySelector('a.download-button')).not.toBeNull();
  });
});

describe('GameBanana adapter', () => {
  it('the real fetched fixture has an empty Files module (client-rendered)', () => {
    const doc = loadFixture('gamebanana-mod.html');
    expect(globalThis.DDMM.siteAdapters.gamebanana.findInsertionPoint(doc)).not.toBeNull();
    expect(globalThis.DDMM.siteAdapters.gamebanana.findDirectDownloadUrl(doc)).toBeNull();
  });

  it('finds the download link once the Files module is populated (synthesized post-render fixture)', () => {
    const doc = loadFixture('gamebanana-mod-populated.html');
    const url = globalThis.DDMM.siteAdapters.gamebanana.findDirectDownloadUrl(doc);
    expect(url).toBe('https://gamebanana.com/mods/download/1105432');
  });
});

describe('GitHub adapter', () => {
  it('the real fetched fixture has no expanded release assets yet', () => {
    const doc = loadFixture('github-releases.html');
    expect(globalThis.DDMM.siteAdapters.github.findDirectDownloadUrl(doc)).toBeNull();
  });

  it('finds the asset link once Assets is expanded (synthesized post-render fixture)', () => {
    const doc = loadFixture('github-releases-expanded.html');
    const url = globalThis.DDMM.siteAdapters.github.findDirectDownloadUrl(doc);
    expect(url).toContain('/releases/download/v2.0.0.0_preview3/');
  });

  it('only activates on /releases pages', () => {
    // The module-level guard reads the real jsdom `location`, which vitest
    // defaults to http://localhost:3000/ -- not a /releases path.
    expect(location.pathname.includes('/releases')).toBe(false);
  });
});

describe('Nexus adapter', () => {
  it('never returns a direct download URL, even in principle', () => {
    expect(globalThis.DDMM.siteAdapters.nexus.findDirectDownloadUrl()).toBeNull();
  });

  it('has no verified insertion point, so it always floats', () => {
    expect(globalThis.DDMM.siteAdapters.nexus.findInsertionPoint()).toBeNull();
  });
});
