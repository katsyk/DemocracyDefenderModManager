import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  BACKGROUND_LIBS,
  buildChromeManifest,
  buildFirefoxManifest,
  extensionIdFromKey,
  SITE_MATCHES,
} from '../../scripts/manifest.mjs';

const BACKGROUND_MAIN = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', '..', 'src', 'background', 'main.js');

// The real key, from /home/rabite/dev/ddmm-secrets/manifest-key.txt (base64
// DER SubjectPublicKeyInfo). Safe to hardcode: it's a *public* key, and this
// exact value is already published in docs/development/bridge-protocol.md
// as the thing that derives the pinned extension id.
const REAL_CHROME_KEY =
  'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAkNbeQACUodr/ZNy48l1PPcc8EW2eqt2cIKPgBBBUF8cnDBvMDIFObISN8aPchgYHtjQ1lwxwMUu3YXJJ2QsYnFzSEIGJax1r6QR9fYFr/AoE7bV7ICY8/AUHShaZ78/yN82olTGquL+9pZp/HvtCBr1nVzD7hxR8nWJVmjiqWmSJIQrH8jSJ331IzxGVv2CzQ0IvC2fDvCszOlSKGhzjPix0vdPwCzIUZDvfR9vhVzZHOusoUICNjq+9Z6t2Ul/KKoQTyBKxJOZYpostva3zgh5z/FXN5Zpl08lcRnUM3YjYSosg3T+RFgzwJhgGiQ8enJwfiJzdL4mmDO3iQPsFLQIDAQAB';
const EXPECTED_ID = 'inomhciahaeeefhgdkiaabdponcfdane';

describe('extensionIdFromKey', () => {
  it('derives the pinned unpacked id from the real manifest key', async () => {
    await expect(extensionIdFromKey(REAL_CHROME_KEY)).resolves.toBe(EXPECTED_ID);
  });

  it('derives a different id from a different key', async () => {
    const other = Buffer.from('not a real der key, just needs to hash differently').toString('base64');
    const id = await extensionIdFromKey(other);
    expect(id).toMatch(/^[a-p]{32}$/);
    expect(id).not.toBe(EXPECTED_ID);
  });
});

const PERMISSIONS = ['nativeMessaging', 'downloads', 'contextMenus', 'storage', 'notifications'];

describe('buildChromeManifest', () => {
  const manifest = buildChromeManifest({ version: '1.2.3', chromeKey: REAL_CHROME_KEY });

  it('sets manifest_version 3 and the version string', () => {
    expect(manifest.manifest_version).toBe(3);
    expect(manifest.version).toBe('1.2.3');
  });

  it('includes the manifest key and derives the pinned id', async () => {
    expect(manifest.key).toBe(REAL_CHROME_KEY);
    await expect(extensionIdFromKey(manifest.key)).resolves.toBe(EXPECTED_ID);
  });

  it('uses a service_worker background and no gecko settings', () => {
    expect(manifest.background).toEqual({ service_worker: 'background/main.js' });
    expect(manifest.browser_specific_settings).toBeUndefined();
  });

  it('omits the key entirely when none is supplied (CI without the secret)', () => {
    const noKey = buildChromeManifest({ version: '1.0.0', chromeKey: undefined });
    expect(noKey.key).toBeUndefined();
  });

  it('requests exactly the specified permissions', () => {
    expect(manifest.permissions.slice().sort()).toEqual(PERMISSIONS.slice().sort());
  });

  it('scopes host_permissions to exactly the five known mod sites', () => {
    expect(manifest.host_permissions.slice().sort()).toEqual(SITE_MATCHES.slice().sort());
  });

  it('never includes the localhost test-only permission by default', () => {
    expect(manifest.host_permissions).not.toContain('http://localhost/*');
    expect(manifest.content_scripts.some((cs) => cs.matches.includes('http://localhost/*'))).toBe(false);
  });

  it('adds the localhost permission and test-fixture content script only when includeLocalhost is true', () => {
    const testManifest = buildChromeManifest({ version: '1.2.3', chromeKey: undefined, includeLocalhost: true });
    expect(testManifest.host_permissions).toContain('http://localhost/*');
    const localhostScript = testManifest.content_scripts.find((cs) => cs.matches.includes('http://localhost/*'));
    expect(localhostScript.js).toContain('content/sites/test-fixture.js');
  });

  it('has a content script for every known site', () => {
    const files = manifest.content_scripts.flatMap((cs) => cs.js);
    expect(files).toContain('content/sites/ayakamods.js');
    expect(files).toContain('content/sites/nexusmods.js');
    expect(files).toContain('content/sites/modworkshop.js');
    expect(files).toContain('content/sites/gamebanana.js');
    expect(files).toContain('content/sites/github.js');
  });
});

describe('buildFirefoxManifest', () => {
  const manifest = buildFirefoxManifest({ version: '1.2.3', geckoId: 'ddmm@katsyk.github.io', minFirefoxVersion: '109.0' });

  it('sets the gecko id and never a Chrome key', () => {
    expect(manifest.browser_specific_settings.gecko.id).toBe('ddmm@katsyk.github.io');
    expect(manifest.browser_specific_settings.gecko.strict_min_version).toBe('109.0');
    expect(manifest.key).toBeUndefined();
  });

  it('uses background.scripts, not a service_worker', () => {
    expect(manifest.background.service_worker).toBeUndefined();
    expect(manifest.background.scripts.at(-1)).toBe('background/main.js');
  });

  // Firefox's event page has no importScripts, so every library main.js
  // needs must be listed ahead of it -- otherwise DDMM is undefined there.
  it('loads every background library before main.js', () => {
    expect(manifest.background).toEqual({ scripts: [...BACKGROUND_LIBS, 'background/main.js'] });
  });

  it('requests exactly the specified permissions and host permissions', () => {
    expect(manifest.permissions.slice().sort()).toEqual(PERMISSIONS.slice().sort());
    expect(manifest.host_permissions.slice().sort()).toEqual(SITE_MATCHES.slice().sort());
  });

  it('matches the Chrome build on every non-browser-specific field', () => {
    const chrome = buildChromeManifest({ version: '1.2.3', chromeKey: undefined });
    expect(manifest.name).toBe(chrome.name);
    expect(manifest.permissions).toEqual(chrome.permissions);
    expect(manifest.content_scripts).toEqual(chrome.content_scripts);
    expect(manifest.icons).toEqual(chrome.icons);
  });
});

describe('background/main.js dependency loading', () => {
  const source = readFileSync(BACKGROUND_MAIN, 'utf8');

  it("importScripts the same libraries, in the same order, as Firefox's background.scripts", () => {
    const call = source.match(/importScripts\(([\s\S]*?)\);/);
    expect(call).not.toBeNull();
    const imported = [...call[1].matchAll(/'\.\.\/(lib\/[^']+)'/g)].map((m) => m[1]);
    expect(imported).toEqual(BACKGROUND_LIBS);
  });

  // Chrome now exposes a `browser` namespace in extension service workers,
  // so gating importScripts on `!browser` skipped it and left DDMM
  // undefined ("Uncaught ReferenceError: DDMM is not defined").
  it('does not gate importScripts on the browser namespace', () => {
    const guard = source.match(/if \(typeof importScripts === 'function'[^)]*\)/);
    expect(guard).not.toBeNull();
    expect(guard[0]).not.toMatch(/browser/);
  });
});
