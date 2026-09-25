#!/usr/bin/env node
/**
 * @file Playwright smoke test: loads the unpacked, test-flavored Chrome
 * build (see `pnpm build:test`) into a real Chromium against a local
 * fixture mod page, and checks that the "Install with DDMM" button
 * appears and reaches a sane state. No native messaging host exists in
 * this environment, so this also asserts the "DDMM not found" path is
 * reached gracefully rather than hanging or throwing.
 *
 * Not part of `pnpm test` (that's the vitest unit suite) -- run explicitly
 * with `pnpm test:e2e`. Requires a real Chromium (uses the system one via
 * CHROMIUM_PATH, falling back to Playwright's bundled build) and, on a
 * headless Linux box with no display, Xvfb:
 *   CHROMIUM_PATH=/usr/bin/chromium xvfb-run -a pnpm test:e2e
 * because loading extensions needs Chromium's classic (non-headless) mode.
 */

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..', '..');
const EXTENSION_DIR = path.join(ROOT, 'dist', 'chrome');
const FIXTURE_FILE = path.join(ROOT, 'test', 'fixtures', 'ayakamods-mod.html');

/** Serves the real (fetched) AyakaMods fixture at http://localhost:<port>/mods/hd2-auto-reload.4084/ . */
async function startFixtureServer() {
  const html = await readFile(FIXTURE_FILE, 'utf8');
  const server = createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(html);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return { server, port };
}

/**
 * Polls a locator's attribute until it matches (plain `document.querySelector`
 * inside `page.waitForFunction` can't see into a shadow root, so this drives
 * the wait from Node instead).
 * @param {import('playwright').Locator} locator
 * @param {string} attribute
 * @param {string} expected
 * @param {number} timeoutMs
 */
async function expectAttributeEventually(locator, attribute, expected, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await locator.getAttribute(attribute);
    if (last === expected) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Timed out waiting for [${attribute}="${expected}"]; last saw "${last}"`);
}

async function main() {
  const { server, port } = await startFixtureServer();
  const userDataDir = path.join(ROOT, '.playwright-profile');

  const context = await chromium.launchPersistentContext(userDataDir, {
    headless: false, // extensions don't load in classic headless Chromium
    executablePath: process.env.CHROMIUM_PATH || undefined,
    args: [
      `--disable-extensions-except=${EXTENSION_DIR}`,
      `--load-extension=${EXTENSION_DIR}`,
      '--no-sandbox', // CI/containers without a user namespace sandbox
    ],
  });

  const pageErrors = [];

  try {
    const page = await context.newPage();
    page.on('pageerror', (err) => pageErrors.push(String(err)));

    // The fixture is served under /mods/hd2-auto-reload.4084/ so
    // sourceFromPageUrl (and hence the reused ayakamods finder functions --
    // see DDMM.content.runTestAdapter / content/sites/test-fixture.js) sees
    // the same URL shape it would on the real site; only the host
    // (localhost, test-build-only permission) differs from production.
    await page.goto(`http://localhost:${port}/mods/hd2-auto-reload.4084/`);

    // The button lives inside an open shadow root (see content/common.js
    // for why 'open' -- the real isolation comes from event.isTrusted, not
    // shadow-root privacy), which Playwright's locators pierce by default.
    const button = page.locator('.ddmm-btn');
    await button.waitFor({ state: 'attached', timeout: 10_000 });
    console.log('PASS: the "Install with DDMM" button appeared on the fixture page.');

    // No native messaging host is registered in this environment, so the
    // button must settle into the "Get DDMM" (unreachable) state instead of
    // hanging on "Checking DDMM…" or throwing. Plain `document.querySelector`
    // (e.g. inside page.waitForFunction) never pierces a shadow root, open
    // or closed -- only Playwright's own locator engine does -- so this
    // polls the locator's `data-state` attribute directly instead.
    await expectAttributeEventually(button, 'data-state', 'unreachable', 15_000);
    const label = await button.textContent();
    if (label !== 'Get DDMM') {
      throw new Error(`Expected the "Get DDMM" label, got "${label}"`);
    }
    console.log('PASS: with no native host present, the button reached the "Get DDMM" state gracefully.');

    // "Get DDMM" is yellow on dark; the emblem must follow the label color.
    const colors = await button.evaluate((btn) => ({
      stroke: getComputedStyle(btn.querySelector('svg path')).stroke,
      color: getComputedStyle(btn).color,
      background: getComputedStyle(btn).backgroundColor,
    }));
    if (colors.stroke !== colors.color || colors.stroke === colors.background) {
      throw new Error(`emblem not visible on "Get DDMM": ${JSON.stringify(colors)}`);
    }
    console.log('PASS: the emblem is visible on the "Get DDMM" button.');
    if (process.env.SHOT_DIR) await button.screenshot({ path: path.join(process.env.SHOT_DIR, 'e2e-button-unreachable.png') });

    await page.waitForTimeout(500);
    if (pageErrors.length > 0) {
      throw new Error(`Unhandled page errors: ${pageErrors.join('; ')}`);
    }
    console.log('PASS: no unhandled page errors.');
  } finally {
    await context.close();
    server.close();
  }
}

main().catch((err) => {
  console.error('FAIL:', err);
  process.exitCode = 1;
});
