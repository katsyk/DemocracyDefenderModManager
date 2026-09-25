# DDMM browser extension

The web half of one-click mod installs for [DDMM](../README.md). Adds an "Install with DDMM" button
to mod pages on AyakaMods, Nexus Mods, ModWorkshop, GameBanana and GitHub (every site treated
equally -- AyakaMods is listed first because it's the community DDMM comes from, not because it gets
special treatment), plus a right-click "Install with DDMM" on any download link, anywhere.

See [PRIVACY.md](./PRIVACY.md) for what the extension does and doesn't collect (nothing), and
[`docs/development/bridge-protocol.md`](../docs/development/bridge-protocol.md) for the native
messaging protocol it speaks to DDMM.

## Quick start

```sh
COREPACK_ENABLE_DOWNLOAD_PROMPT=0 pnpm install
pnpm test    # vitest unit suite
pnpm build   # -> dist/chrome/, dist/firefox/, and both zips
pnpm lint    # web-ext lint against the Firefox build
```

### Loading it in a browser

**Chrome / Edge / Brave:** `pnpm build`, then go to `chrome://extensions`, enable Developer Mode,
"Load unpacked", and select `extension/dist/chrome/`.

**Firefox:** `pnpm build`, then go to `about:debugging#/runtime/this-firefox`, "Load Temporary
Add-on...", and select `extension/dist/firefox/manifest.json`. This only lasts until Firefox
restarts (see `docs/using/extension-install.snippet.md` for the AMO-signed path, coming later).

## Why this package is standalone, not a pnpm workspace member

The root `pnpm-workspace.yaml` has no `packages:` list -- the repo root is a single package, not a
workspace, today. Adding `extension` as a workspace member would mean the root `pnpm-lock.yaml` has
to account for its dependencies too, and `pnpm install --frozen-lockfile` at the root (what CI's main
`test` job runs) would fail the moment this package's dependencies didn't already match that lockfile.
Rather than touch the root lockfile (owned by the desktop app's own build), `extension/` is a fully
standalone package: its own `package.json`, its own `pnpm-lock.yaml`, its own `pnpm-workspace.yaml`
(just the `allowBuilds` entries pnpm's install needs for `esbuild`/`spawn-sync`, mirroring the root
one). The root's `pnpm install --frozen-lockfile` never looks inside `extension/`, so it's completely
unaffected. CI has its own `extension` job that runs `pnpm install` etc. from inside `extension/`.

## Why classic scripts, not ES modules

The task brief asks for ES modules. In practice, MV3 content scripts can't be declared as ES modules
in the manifest (there's no `content_scripts[].type` field -- only `background.service_worker` can be
`"type": "module"` on Chrome), and the usual workaround (a tiny classic-script loader that
`import()`s a real module) needs those modules listed in `web_accessible_resources` to be fetchable
from a content script's isolated world. That's extra manifest surface, and one more thing a store
reviewer has to reason about, for no real benefit here.

Instead, every source file (background, content scripts, popup) is a classic script that attaches
its exports to a shared `globalThis.DDMM` namespace (see `src/lib/browser-shim.js` for the pattern).
Manifest `content_scripts[].js` arrays and the background's file list are just ordered lists of these
classic scripts sharing one global scope -- exactly like a handful of `<script>` tags. Chrome's
service worker background loads the same `lib/*.js` files via `importScripts()`; Firefox's
`background.scripts` lists them directly. JSDoc typedefs throughout give editors real type
information without a build step. See `src/lib/*.js` for the pattern used everywhere.

## Architecture

```
src/
  lib/                  Pure logic, no DOM/browser-API side effects at import time (mostly):
    browser-shim.js       chrome.* / browser.* -> one promise-based DDMM.browserApi
    sources.js             mod-page URL parsing, ported 1:1 from src-tauri/src/sources.rs
    errors.js               protocol error code -> friendly message table
    protocol-client.js     native-messaging client: request ids, timeouts, reconnect, serialized installs
    capture.js              download-capture matching (site hosts, archive detection, arm/expire)
    button-state.js         the button's state machine
    downloads-adapter.js   Chrome/Firefox DownloadItem differences, normalized
    storage.js               storage.local schema (auto-capture toggles, after-install override, recent installs)
  background/
    main.js                the service worker (Chrome) / event page (Firefox): owns the one
                            ProtocolClient, the downloads.onChanged capture logic, the context menu,
                            and the message router content scripts/popup talk to
  content/
    common.js               the shadow-DOM button widget + the per-page driver (query, click, install)
    sites/*.js               one file per site: URL-shape detection is shared (sources.js); each file
                            supplies findInsertionPoint / findDirectDownloadUrl / scrapeVersion
  popup/                   connection status, per-site auto-capture toggle, after-install override,
                            recent installs
scripts/
  manifest.mjs             pure functions building the Chrome and Firefox manifest objects
  build.mjs                 copies src/ + icons/ into dist/<browser>/, writes manifest.json, zips
  lint.mjs                   runs web-ext lint against the Firefox build
test/
  fixtures/                 trimmed real (and clearly-labeled synthesized) HTML from each site
  unit/                     vitest suite
  e2e/smoke.mjs             Playwright smoke test against a local fixture page (see below)
```

## Site findings (what's real DOM vs. fallback)

Live pages were fetched with curl during development; see `test/fixtures/` for what came back and
each `src/content/sites/*.js` file's header comment for the detailed reasoning. Summary:

| Site | Real DOM obtained? | Install path |
| --- | --- | --- |
| AyakaMods | Yes | Inline button in the sidebar; guest downloads are login-gated (no `<a>`, just a disabled placeholder), so it arms capture and lets the user log in and click AyakaMods' own download control |
| ModWorkshop | Yes (server-rendered) | Inline button; `a.download-button[href]` is a direct file link straight to `storage.modworkshop.net` -- no capture needed |
| GameBanana | Partially -- the `<module data-module="Files">` placeholder is real, but GameBanana fills it with actual download links via client-side JS after load, which curl never sees | Inline button anchored to that module; the direct-link search happens at click time (by then the site's own JS has rendered it in a real browser) |
| GitHub | Yes, including confirming what's *not* there -- release assets always lazy-load through `<include-fragment src=".../releases/expanded_assets/<tag>">`, on both the release list and single-tag views | Inline button once Assets is expanded and a `/releases/download/` link exists at click time; otherwise capture-arm with a hint to expand Assets. The context-menu "Install with DDMM" on the asset link works on this site regardless |
| Nexus Mods | No -- every attempt (multiple mod ids, multiple user agents, browser-like headers) hit Cloudflare's interactive challenge, never the real page | Floating fallback button, always capture-arm. This is also a deliberate policy, not just a DOM limitation: free Nexus downloads require a manual "Slow download" click, and the extension must never automate that click |

Two fixtures are clearly marked as **synthesized, not fetched** (`gamebanana-mod-populated.html`,
`github-releases-expanded.html`): they approximate what the DOM looks like once each site's own
client-side rendering has run, so the relevant `findDirectDownloadUrl()` codepath has a unit test.
They're not claimed as verified captures.

## Testing

- `pnpm test` -- vitest unit suite (103 tests): URL/id parsing (mirrors the exact test cases in
  `src-tauri/src/sources.rs`), download-capture matching, the native-messaging protocol client
  (fake port: ids, timeouts, reconnect, serialized installs), the button state machine, manifest
  build output per browser (including that the Chrome `key` derives the pinned extension id), and
  the site adapters' DOM-parsing functions against the fixtures above.
- `pnpm lint` -- `web-ext lint` against the Firefox build.
- `pnpm test:e2e` -- Playwright, loading the real unpacked extension (built with the test-only
  `--test` flag, which adds an `http://localhost/*` host permission and a
  `content/sites/test-fixture.js` adapter -- never present in a release build; see
  `scripts/manifest.mjs`'s `includeLocalhost`) against a local HTTP server serving the AyakaMods
  fixture. Confirms the button appears in the page and, since no native host exists in this
  environment, that it reaches the "Get DDMM" state gracefully rather than hanging or throwing.
  Needs a real Chromium and (headless Linux) Xvfb, since extensions don't load in classic headless
  mode:
  ```sh
  CHROMIUM_PATH=/usr/bin/chromium xvfb-run -a pnpm test:e2e
  ```

## The Chrome manifest key

Chrome/Edge/Brave's unpacked extension id (`inomhciahaeeefhgdkiaabdponcfdane`) is pinned by putting
the corresponding public key in the manifest's `key` field. That key is a secret file, never in this
repo. `scripts/build.mjs` looks for it as `DDMM_MANIFEST_KEY` (the base64 value itself) or a file at
`DDMM_MANIFEST_KEY_FILE` (defaulting to `/home/rabite/dev/ddmm-secrets/manifest-key.txt` for local
dev). If neither is present -- e.g. in CI -- the build still succeeds, just without a pinned id.
