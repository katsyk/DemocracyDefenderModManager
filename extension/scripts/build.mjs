#!/usr/bin/env node
/**
 * @file Builds extension/dist/chrome/ and extension/dist/firefox/ from the
 * single extension/src/ tree, then zips each into
 * ddmm-extension-<browser>-<version>.zip in extension/dist/.
 *
 * No bundler: this just copies files and writes one manifest.json per
 * browser (see manifest.mjs for the actual per-browser differences).
 *
 * The Chrome manifest `key` (which pins the unpacked extension id to
 * inomhciahaeeefhgdkiaabdponcfdane) is a secret never committed to this
 * repo. The build looks for it, in order:
 *   1. `DDMM_MANIFEST_KEY` env var (the base64 DER public key itself).
 *   2. A file at `DDMM_MANIFEST_KEY_FILE` (default:
 *      /home/rabite/dev/ddmm-secrets/manifest-key.txt).
 * If neither is available (e.g. on CI, or a contributor's machine), the
 * build still succeeds -- it just omits `key`, so Chrome assigns a random
 * id for that unpacked build instead of the pinned one. This is what keeps
 * `pnpm build` green in CI without the secret present.
 */

import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import archiver from 'archiver';

import { buildChromeManifest, buildFirefoxManifest } from './manifest.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..');
const SRC = path.join(ROOT, 'src');
const ICONS = path.join(ROOT, 'icons');
const DIST = path.join(ROOT, 'dist');

const DEFAULT_KEY_FILE = '/home/rabite/dev/ddmm-secrets/manifest-key.txt';
const GECKO_ID = 'ddmm@katsyk.github.io';
const MIN_FIREFOX_VERSION = '109.0';

/** @returns {Promise<string>} The extension version, from package.json. */
async function readVersion() {
  const pkg = JSON.parse(await readFile(path.join(ROOT, 'package.json'), 'utf8'));
  return pkg.version;
}

/** @returns {Promise<string|undefined>} The Chrome manifest key, if available. */
async function readChromeKey() {
  if (process.env.DDMM_MANIFEST_KEY) return process.env.DDMM_MANIFEST_KEY.trim();
  const keyFile = process.env.DDMM_MANIFEST_KEY_FILE || DEFAULT_KEY_FILE;
  if (existsSync(keyFile)) {
    return (await readFile(keyFile, 'utf8')).trim();
  }
  return undefined;
}

/**
 * @param {string} outDir
 * @param {object} manifest
 * @param {boolean} testBuild - When false (every release build), the
 *   localhost-only test fixture adapter is excluded from the copied tree
 *   entirely -- it must never ship, not even unreferenced.
 */
async function writeBrowserBuild(outDir, manifest, testBuild) {
  await rm(outDir, { recursive: true, force: true });
  await mkdir(outDir, { recursive: true });
  await cp(SRC, outDir, {
    recursive: true,
    filter: (src) => testBuild || !src.endsWith(path.join('content', 'sites', 'test-fixture.js')),
  });
  await cp(ICONS, path.join(outDir, 'icons'), {
    recursive: true,
    filter: (src) => !src.endsWith('.svg'), // ship PNGs only, not the source SVG
  });
  await writeFile(path.join(outDir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
}

/**
 * @param {string} dir - Directory to zip (its contents become the zip root).
 * @param {string} zipPath
 * @returns {Promise<void>}
 */
function zipDirectory(dir, zipPath) {
  return new Promise((resolve, reject) => {
    const output = createWriteStream(zipPath);
    const archive = archiver('zip', { zlib: { level: 9 } });
    output.on('close', resolve);
    archive.on('error', reject);
    archive.pipe(output);
    archive.directory(dir, false);
    archive.finalize();
  });
}

async function main() {
  const testBuild = process.argv.includes('--test');
  const version = await readVersion();
  const chromeKey = await readChromeKey();

  await mkdir(DIST, { recursive: true });

  const chromeDir = path.join(DIST, 'chrome');
  const firefoxDir = path.join(DIST, 'firefox');

  await writeBrowserBuild(
    chromeDir,
    buildChromeManifest({ version, chromeKey, includeLocalhost: testBuild }),
    testBuild,
  );
  await writeBrowserBuild(
    firefoxDir,
    buildFirefoxManifest({ version, geckoId: GECKO_ID, minFirefoxVersion: MIN_FIREFOX_VERSION }),
    testBuild,
  );

  if (testBuild) {
    console.log('Built with --test: localhost host permission + test-fixture.js included (Chrome only). Never do this for a release build.');
    return; // skip zipping; the smoke test loads dist/chrome/ unpacked directly.
  }

  const chromeZip = path.join(DIST, `ddmm-extension-chrome-${version}.zip`);
  const firefoxZip = path.join(DIST, `ddmm-extension-firefox-${version}.zip`);
  await zipDirectory(chromeDir, chromeZip);
  await zipDirectory(firefoxDir, firefoxZip);

  if (chromeKey) {
    const der = Buffer.from(chromeKey, 'base64');
    const hash = createHash('sha256').update(der).digest('hex').slice(0, 32);
    const id = hash.replace(/./g, (c) => String.fromCharCode(97 + parseInt(c, 16)));
    console.log(`Chrome build: manifest key present, unpacked id = ${id}`);
  } else {
    console.log('Chrome build: no manifest key found (DDMM_MANIFEST_KEY / DDMM_MANIFEST_KEY_FILE) -- built without a pinned id.');
  }
  console.log(`Wrote ${chromeZip}`);
  console.log(`Wrote ${firefoxZip}`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
