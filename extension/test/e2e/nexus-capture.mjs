#!/usr/bin/env node
/**
 * @file Real-Chromium check that a Nexus download started by the user's
 * own tool (not Nexus's countdown) is installed.
 *
 * A local HTTPS server plays both www.nexusmods.com (a fake mod file page
 * whose countdown never finishes during the test) and a Nexus CDN host
 * (supporter-files.nexus-cdn.com), mapped with --host-resolver-rules. A fake
 * native messaging host stands in for DDMM and records every `install`. A
 * small injected script plays the part of a user's userscript: it starts a
 * CDN download directly, without the countdown.
 *
 * Scenarios:
 *   A. The download starts on page load and finishes *before* the user
 *      clicks "Install with DDMM". The click must install it.
 *   B. The user clicks "Install with DDMM" first; the download then starts
 *      in a new tab opened with `noreferrer` (so no referrer). It must be
 *      installed, and the button must show "Click Download on this page…"
 *      at once instead of waiting for the countdown.
 *
 * Needs a real (non-headless) Chromium and a display (Xvfb is fine), plus
 * `openssl` for the throwaway certificate:
 *   pnpm build && CHROMIUM_PATH=/usr/bin/chromium xvfb-run -a node test/e2e/nexus-capture.mjs
 * EXTENSION_DIR overrides the unpacked extension (default dist/chrome).
 */

import { createServer } from 'node:https';
import { execFileSync, spawn } from 'node:child_process';
import { chmod, mkdir, mkdtemp, readdir, rm, writeFile, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..', '..');
const EXTENSION_DIR = path.resolve(process.env.EXTENSION_DIR || path.join(ROOT, 'dist', 'chrome'));
const EXTENSION_ID = 'inomhciahaeeefhgdkiaabdponcfdane';
const HOST_NAME = 'io.github.katsyk.ddmm';

const MOD_ID = '123';
const FILE_PAGE_PATH = `/helldivers2/mods/${MOD_ID}?tab=files&file_id=456`;
const cdnPath = (tag) => `/6119/${MOD_ID}/Cool Mod ${tag}-${MOD_ID}-1-0-1700000000.zip?md5=abc&expires=1700003600&user_id=1`;

// A minimal valid (empty) zip: just the end-of-central-directory record.
const EMPTY_ZIP = Buffer.from([0x50, 0x4b, 0x05, 0x06, ...new Array(18).fill(0)]);

const FILE_PAGE_HTML = `<!doctype html><html><head><title>Cool Mod - Helldivers 2 Nexus</title></head>
<body><h1>Cool Mod</h1>
<p>Your download will start in <span id="t">60</span> seconds</p>
<button id="slow" disabled>Slow download</button>
<script>
  let n = 60;
  setInterval(() => { if (n > 0) document.getElementById('t').textContent = --n; }, 1000);
</script></body></html>`;

function makeCert(dir) {
  const key = path.join(dir, 'key.pem');
  const cert = path.join(dir, 'cert.pem');
  execFileSync('openssl', [
    'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=localhost',
    '-keyout', key, '-out', cert,
  ], { stdio: 'ignore' });
  return { key, cert };
}

async function startServer(dir) {
  const { key, cert } = makeCert(dir);
  const server = createServer({ key: await readFile(key), cert: await readFile(cert) }, (req, res) => {
    const host = (req.headers.host || '').split(':')[0];
    if (host === 'www.nexusmods.com') {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(FILE_PAGE_HTML);
    } else if (host === 'supporter-files.nexus-cdn.com' && req.url.startsWith(`/6119/${MOD_ID}/`)) {
      const name = decodeURIComponent(req.url.split('?')[0].split('/').pop());
      res.writeHead(200, {
        'content-type': 'application/zip',
        'content-disposition': `attachment; filename="${name}"`,
        'content-length': EMPTY_ZIP.length,
      });
      res.end(EMPTY_ZIP);
    } else {
      res.writeHead(404);
      res.end();
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return server;
}

/** A fake DDMM: a native messaging host that logs each request as a JSON line. */
async function installFakeHost(dir, userDataDir) {
  const log = path.join(dir, 'host-requests.jsonl');
  const script = path.join(dir, 'fake-host.mjs');
  await writeFile(script, `#!${process.execPath}
import { appendFileSync } from 'node:fs';
let buf = Buffer.alloc(0);
function send(obj) {
  const body = Buffer.from(JSON.stringify(obj));
  const len = Buffer.alloc(4); len.writeUInt32LE(body.length);
  process.stdout.write(Buffer.concat([len, body]));
}
process.stdin.on('data', (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  while (buf.length >= 4) {
    const n = buf.readUInt32LE(0);
    if (buf.length < 4 + n) break;
    const msg = JSON.parse(buf.subarray(4, 4 + n).toString());
    buf = buf.subarray(4 + n);
    appendFileSync(${JSON.stringify(log)}, JSON.stringify(msg) + '\\n');
    if (msg.type === 'hello') send({ id: msg.id, ok: true, type: 'hello', appVersion: 'test', protocol: 1, afterInstall: 'library', gameFound: true });
    else if (msg.type === 'query') send({ id: msg.id, ok: true, type: 'queryResult', installed: false, mod: null, updateAvailable: null });
    else if (msg.type === 'install') send({ id: msg.id, ok: true, type: 'installed', mod: { name: 'Cool Mod', source: { provider: 'nexus', id: '${MOD_ID}' }, version: '1.0' }, updated: false, deployed: false, warnings: [] });
    else send({ id: msg.id, ok: false, error: { code: 'UNSUPPORTED', message: '' } });
  }
});
process.stdin.on('end', () => process.exit(0));
`);
  await chmod(script, 0o755);
  const manifestDir = path.join(userDataDir, 'NativeMessagingHosts');
  await mkdir(manifestDir, { recursive: true });
  await writeFile(path.join(manifestDir, `${HOST_NAME}.json`), JSON.stringify({
    name: HOST_NAME,
    description: 'DDMM e2e fake host',
    path: script,
    type: 'stdio',
    allowed_origins: [`chrome-extension://${EXTENSION_ID}/`],
  }));
  return log;
}

async function readInstalls(log) {
  if (!existsSync(log)) return [];
  return (await readFile(log, 'utf8')).split('\n').filter(Boolean).map((l) => JSON.parse(l)).filter((m) => m.type === 'install');
}

async function waitFor(fn, what, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 200));
  }
}

function freePort() {
  return new Promise((resolve) => {
    const s = net.createServer();
    s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
  });
}

async function main() {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'ddmm-nexus-e2e-'));
  const userDataDir = path.join(dir, 'profile');
  const downloadDir = path.join(dir, 'downloads');
  await mkdir(path.join(userDataDir, 'Default'), { recursive: true });
  await mkdir(downloadDir);
  await writeFile(path.join(userDataDir, 'Default', 'Preferences'), JSON.stringify({
    download: { default_directory: downloadDir, prompt_for_download: false, directory_upgrade: true },
  }));
  const hostLog = await installFakeHost(dir, userDataDir);
  const server = await startServer(dir);
  const { port } = server.address();
  const cdpPort = await freePort();

  // Launched directly (not via Playwright's launcher) so the browser's own
  // download handling -- and so chrome.downloads -- is untouched.
  const browser = spawn(process.env.CHROMIUM_PATH || chromium.executablePath(), [
    `--user-data-dir=${userDataDir}`,
    `--remote-debugging-port=${cdpPort}`,
    `--disable-extensions-except=${EXTENSION_DIR}`,
    `--load-extension=${EXTENSION_DIR}`,
    `--host-resolver-rules=MAP www.nexusmods.com 127.0.0.1:${port}, MAP supporter-files.nexus-cdn.com 127.0.0.1:${port}`,
    '--ignore-certificate-errors',
    '--no-first-run', '--no-default-browser-check', '--password-store=basic', '--no-sandbox', '--disable-gpu',
    'about:blank',
  ], { stdio: 'ignore' });

  let failed = false;
  const shots = process.env.SHOT_DIR;
  try {
    const cdp = await waitFor(async () => {
      try {
        return await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
      } catch {
        return null;
      }
    }, 'Chromium DevTools');
    const context = cdp.contexts()[0];
    // Playwright takes over download handling when it connects; hand it
    // back to the browser (plain "save to this folder"), as for a user.
    const browserSession = await cdp.newBrowserCDPSession();
    await browserSession.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: downloadDir });
    await waitFor(() => context.serviceWorkers().some((w) => w.url().includes(EXTENSION_ID)), 'the extension service worker');

    // ---- Scenario A: the download finishes before the button is clicked.
    const pageA = await context.newPage();
    await pageA.addInitScript((url) => {
      // The "userscript": start the CDN download straight away.
      if (location.hostname === 'www.nexusmods.com') setTimeout(() => { location.href = url; }, 50);
    }, `https://supporter-files.nexus-cdn.com${cdnPath('A')}`);
    await pageA.goto(`https://www.nexusmods.com${FILE_PAGE_PATH}`);
    await waitFor(async () => (await readdir(downloadDir)).some((f) => f.includes('Cool Mod A') && !f.endsWith('.crdownload')), 'download A on disk');
    const buttonA = pageA.locator('.ddmm-btn');
    await waitFor(async () => (await buttonA.getAttribute('data-state')) === 'install', 'button A ready');
    if (shots) await pageA.screenshot({ path: path.join(shots, 'e2e-a1-before-click.png') });
    if ((await readInstalls(hostLog)).length !== 0) throw new Error('installed before the user clicked');
    await buttonA.click();
    const installsA = await waitFor(async () => {
      const list = await readInstalls(hostLog);
      return list.length >= 1 ? list : null;
    }, 'install A (download finished before the click)', 10_000);
    if (!installsA[0].file.includes('Cool Mod A')) throw new Error(`wrong file: ${installsA[0].file}`);
    if (installsA[0].pageUrl !== `https://www.nexusmods.com${FILE_PAGE_PATH}`) throw new Error(`wrong pageUrl: ${installsA[0].pageUrl}`);
    await waitFor(async () => (await buttonA.getAttribute('data-state')) === 'installed', 'button A installed');
    if (shots) await pageA.screenshot({ path: path.join(shots, 'e2e-a2-installed.png') });
    console.log(`PASS A: a download that finished before the click was installed (${installsA[0].file}).`);

    // ---- Scenario B: armed first, then a no-referrer download in a new tab.
    const pageB = await context.newPage();
    await pageB.goto(`https://www.nexusmods.com${FILE_PAGE_PATH}`);
    const buttonB = pageB.locator('.ddmm-btn');
    await waitFor(async () => (await buttonB.getAttribute('data-state')) === 'install', 'button B ready');
    await buttonB.click();
    await waitFor(async () => (await buttonB.getAttribute('data-state')) === 'waiting', 'button B "Click Download on this page…"', 3_000);
    const labelB = await buttonB.textContent();
    if (labelB !== 'Click Download on this page…') throw new Error(`unexpected label: ${labelB}`);
    if (shots) await pageB.screenshot({ path: path.join(shots, 'e2e-b1-waiting.png') });
    await pageB.evaluate((url) => {
      const a = document.createElement('a');
      a.href = url;
      a.target = '_blank';
      a.rel = 'noreferrer noopener';
      document.body.appendChild(a);
      a.click();
    }, `https://supporter-files.nexus-cdn.com${cdnPath('B')}`);
    const installsB = await waitFor(async () => {
      const list = await readInstalls(hostLog);
      return list.length >= 2 ? list : null;
    }, 'install B (new tab, no referrer, countdown still running)');
    if (!installsB[1].file.includes('Cool Mod B')) throw new Error(`wrong file: ${installsB[1].file}`);
    await waitFor(async () => (await buttonB.getAttribute('data-state')) === 'installed', 'button B installed');
    if (shots) await pageB.screenshot({ path: path.join(shots, 'e2e-b2-installed.png') });
    console.log(`PASS B: a no-referrer download from another tab was installed while the countdown still ran (${installsB[1].file}).`);
    await cdp.close();
  } catch (err) {
    failed = true;
    console.error('FAIL:', err.message);
    console.error('host requests:', JSON.stringify(await readInstalls(hostLog)));
  } finally {
    browser.kill();
    server.close();
    await new Promise((r) => setTimeout(r, 500));
    if (process.env.KEEP_TMP) console.log(`kept ${dir}`);
    else await rm(dir, { recursive: true, force: true }).catch(() => {});
  }
  process.exitCode = failed ? 1 : 0;
}

main();
