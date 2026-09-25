# Browser bridge protocol (v1)

This page is the contract between the **DDMM browser extension**, the **native messaging host**, and the
**running DDMM app**. It also defines the `ddmm://` link format. Both sides are implemented against this document.
If you change the protocol, change this page in the same pull request and bump `protocol` if it's incompatible.

## Goals

- **One click from any mod site.** AyakaMods, Nexus Mods, ModWorkshop, GameBanana, GitHub and any other site are
  equal. No site is special-cased in a way that makes another site second-class.
- **The browser downloads, DDMM installs.** Login-gated downloads work because the *browser* fetches the file with the
  user's own session. DDMM never sees, asks for, or stores credentials, cookies or API keys for any site.
- **Web pages can't drive DDMM.** Only the extension (via native messaging) can hand DDMM a file. `ddmm://` links, which
  any page can trigger, always require explicit confirmation in DDMM.

## Components

```
 mod site page ──(content script button / context menu / download capture)──► extension (background)
                                                                                   │ chrome.runtime.connectNative
                                                                                   ▼
                                               native messaging host  =  ddmm executable in host mode
                                                                                   │ 127.0.0.1 TCP + token
                                                                                   ▼
                                                                            running DDMM app
```

### Identifiers

| What | Value |
| --- | --- |
| Native messaging host name | `io.github.katsyk.ddmm` |
| Chrome/Edge/Brave extension ID (unpacked, from the manifest `key`) | `inomhciahaeeefhgdkiaabdponcfdane` |
| Firefox extension ID (`browser_specific_settings.gecko.id`) | `ddmm@katsyk.github.io` |
| Deep-link scheme | `ddmm://` |
| Protocol version | `1` |

A Chrome Web Store ID will be added to `allowed_origins` once the extension is published there.

## Native messaging host

The host is the **same `ddmm` executable** started in *host mode*. The browser launches it with:

- Chromium browsers: first argument `chrome-extension://<id>/` (Windows may add `--parent-window=<n>`).
- Firefox: arguments `<path to host manifest> <extension id>`.

`ddmm` enters host mode when argv matches either shape, and must then **never open a window**. It speaks standard native
messaging on stdin/stdout: each message is a 32-bit **native-endian** (little-endian on all supported targets) length
followed by UTF-8 JSON. Messages from the host to the browser must be ≤ 1 MB; messages from the browser are capped at
64 MB by the browser but DDMM rejects anything > 1 MB.

The host is a **relay**. It validates framing, forwards each JSON message to the running app, and writes back each
reply. It adds the caller origin (`chrome-extension://…` or the Firefox ID) as `origin` to every forwarded request, and
the app rejects origins not in its allowlist.

### Reaching the running app

When DDMM starts it listens on `127.0.0.1:<random free port>` (never `0.0.0.0`) and writes
`<data folder>/bridge.json`:

```json
{ "port": 51234, "token": "<32 random bytes, hex>", "pid": 4242, "protocol": 1 }
```

The file is created with owner-only permissions (0600 on Unix, and inherited per-user ACLs under the user profile on
Windows) and deleted on clean exit. The token is regenerated on every start.

The host resolves the data folder with the **same logic as the app** (portable marker / app-data), reads `bridge.json`,
connects, and sends `{"hello": "<token>"}` followed by a newline. The app replies `{"ok": true}` and closes the connection
on a wrong token. After that, both sides exchange **newline-delimited JSON** (one object per line).

If `bridge.json` is missing, stale (connection refused), or its `pid` is gone, the host **starts DDMM normally** (a
detached process, so the GUI appears) and polls for a fresh `bridge.json` for up to 20 s before replying with the
error `APP_NOT_RUNNING`.

## Messages

Every request has a string `id` chosen by the extension. Every reply echoes the `id`. Unknown `type` → error
`UNSUPPORTED`. Unknown fields are ignored (forward compatibility).

### `hello`

```json
{ "id": "1", "type": "hello", "extensionVersion": "1.0.0", "browser": "chrome" }
```
Reply:
```json
{ "id": "1", "ok": true, "type": "hello", "appVersion": "2.0.0-rc.4", "protocol": 1,
  "afterInstall": "deploy", "gameFound": true }
```

### `install`

The extension sends this after a download **has completed** in the browser.

```json
{ "id": "2", "type": "install",
  "file": "C:\\Users\\me\\Downloads\\Cool Mod-4084-1-0.zip",
  "pageUrl": "https://ayakamods.com/mods/cool-mod.4084/",
  "downloadUrl": "https://ayakamods.com/mods/cool-mod.4084/download",
  "pageVersion": "1.0",
  "afterInstall": null }
```

- `file`: absolute path of the finished download. The app requires a regular file (not a symlink), an allowed archive
  type by **magic bytes**, and ≤ 2 GiB. The app never deletes or moves it.
- `pageUrl`: the mod page the user was on (the source of truth for `Sources`; parsed with the same rules as
  `source_from_page_url`). It may be null for context-menu installs from arbitrary pages; then `downloadUrl` host detection
  is used.
- `pageVersion`: optional version scraped from the page (for example AyakaMods JSON-LD `softwareVersion`), recorded in the
  origin sidecar.
- `afterInstall`: `null` means "use the app setting". Otherwise one of `"library"`, `"profile"` or `"deploy"`.

If a mod from the same source (provider + id) is already installed, the app performs an **in-place update** (keeping the
GUID and profile config, like "Update from …") instead of failing with a duplicate.

**Permission check (app side):** the first time a site (registrable domain of `pageUrl`/`downloadUrl`) sends an install,
DDMM asks: *"Allow the DDMM browser extension to install mods from **ayakamods.com**?"* with **Always allow**, **Just this
once** and **Deny**. "Always allow" is stored in settings (`BridgeAllowedSites`) and can be revoked in Settings. The prompt
is skipped for allowed sites; that's what makes it one click.

Reply on success:
```json
{ "id": "2", "ok": true, "type": "installed",
  "mod": { "guid": "…", "name": "Cool Mod", "source": { "provider": "ayakamods", "id": "4084" }, "version": "1.0" },
  "updated": false, "addedToProfile": "Default", "deployed": true, "warnings": [] }
```

### `query`

Lets the extension label its button ("Install with DDMM" / "Installed ✓" / "Update with DDMM").

```json
{ "id": "3", "type": "query", "pageUrl": "https://ayakamods.com/mods/cool-mod.4084/", "pageVersion": "1.1" }
```
Reply:
```json
{ "id": "3", "ok": true, "type": "queryResult", "installed": true,
  "mod": { "guid": "…", "name": "Cool Mod", "installedVersion": "1.0" }, "updateAvailable": true }
```
`updateAvailable` is `null` when either version is unknown. It is never guessed.

### `status`

```json
{ "id": "4", "type": "status" }
```
Reply: `{ "id": "4", "ok": true, "type": "status", "gameFound": true, "activeProfile": "Default", "modCount": 12, "busy": false }`

### Errors

```json
{ "id": "2", "ok": false, "error": { "code": "DECLINED", "message": "You chose not to install this mod." } }
```

| Code | Meaning |
| --- | --- |
| `APP_NOT_RUNNING` | Host couldn't start or reach DDMM |
| `BAD_REQUEST` | Malformed message / missing field / message too large |
| `UNSUPPORTED` | Unknown message type or protocol version |
| `FORBIDDEN_ORIGIN` | Caller isn't an allowed extension |
| `DECLINED` | The user answered Deny / closed the prompt |
| `NOT_ARCHIVE` | File isn't a zip/7z/rar |
| `UNSAFE_ARCHIVE` | Archive failed path-traversal / symlink validation |
| `FILE_NOT_FOUND` | `file` doesn't exist or isn't a regular file |
| `GAME_NOT_FOUND` | `afterInstall: deploy` but the game path isn't set/valid (mod is still installed) |
| `DEPLOY_FAILED` | Installed, but deploy failed (mod is still installed) |
| `BUSY` | Another install/deploy is running; retry |
| `INTERNAL` | Anything else (message says what) |

Installs are serialized in the app; concurrent requests queue (up to 20), and beyond that they get `BUSY`.

## `ddmm://` links

Any web page (a mod site, a mod author's page, a Discord message) can link:

```
ddmm://install?url=<percent-encoded https URL>
```

`url` is either a mod page (handled like **Add URL**: browser handoff for login-gated sites) or a direct https download.
DDMM **always** shows a confirmation ("A link wants to install a mod from **example.com**. Install?") with no
"always allow". Only `https` URLs are accepted. Anything else in the link is ignored. `ddmm://open` just focuses DDMM.

Deep links reach an already-running DDMM through the single-instance mechanism. A second window is never opened.

## Security checklist

- TCP listener bound to loopback only, random port, per-run 256-bit token, constant-time token compare.
- `bridge.json` is owner-only, deleted on exit.
- Host mode never opens a GUI and only relays. Message size limits are enforced both ways.
- App-side origin allowlist (the extension IDs above).
- Per-site "Always allow" consent for extension installs. `ddmm://` always confirms.
- Content-script buttons act only on trusted clicks (`event.isTrusted`), so a page can't script-click them.
- Every install still goes through archive validation (paths, symlinks, magic bytes, size cap).
- No credentials, cookies or tokens for any mod site ever cross the bridge.
