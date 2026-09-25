#!/usr/bin/env node
/**
 * @file Runs `web-ext lint` against the built Firefox extension, building
 * it first if needed. Reports and exits cleanly (without failing the
 * `lint` script itself) if `web-ext` isn't available, per the task's
 * "report its output" instruction rather than treating it as a hard build
 * dependency.
 */

import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..');
const FIREFOX_DIR = path.join(ROOT, 'dist', 'firefox');

function run(cmd, args, opts = {}) {
  return spawnSync(cmd, args, { stdio: 'inherit', cwd: ROOT, ...opts });
}

if (!existsSync(path.join(FIREFOX_DIR, 'manifest.json'))) {
  console.log('dist/firefox not built yet -- running build first.');
  const build = run(process.execPath, [path.join(__dirname, 'build.mjs')]);
  if (build.status !== 0) {
    console.error('Build failed; cannot lint.');
    process.exit(build.status ?? 1);
  }
}

const webExtBin = path.join(ROOT, 'node_modules', '.bin', 'web-ext');
const result = existsSync(webExtBin)
  ? run(webExtBin, [
      'lint',
      '--source-dir',
      FIREFOX_DIR,
      '--pretty',
      '--self-hosted', // this build isn't going through AMO signing yet; don't require an AMO-only warning-free result
    ])
  : { error: new Error(`${webExtBin} not found`) };

if (result.error) {
  console.warn(`web-ext isn't available (${result.error.message}); skipping lint. See extension/README.md.`);
  process.exit(0);
}

process.exit(result.status ?? 0);
