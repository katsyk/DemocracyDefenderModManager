---
title: Security
---

# Security

## No logins, and one optional key

DDMM never asks for, stores, or transmits a username or password for any mod site. Sites that require being
logged in to download hand the actual download off to your own browser, where your own session (and your own
login) does the work — see [Mod sites](../using/mod-sites.md). Update checks read public page/API information.

The single exception is optional: you can add your own **personal Nexus Mods API key** so DDMM can check Nexus
mods for updates. It is never required, and:

- it's stored in your OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service), or, on a
  Linux desktop without one, in an owner-only (`0600`) file in DDMM's data folder, and Settings tells you which;
- it's never written to `settings.json` and never logged (errors that could contain it are scrubbed first);
- it's only ever sent to `https://api.nexusmods.com` (redirects are not followed, so it can't be forwarded
  elsewhere), and never crosses the browser bridge;
- it's never used to download anything. Nexus updates always go through your browser.

See [Updating mods](../using/updating-mods.md#nexus-mods-and-the-optional-api-key).

## Archive extraction hardening

Every archive (`.zip`, `.7z`, `.rar`) is checked before any of its contents are written to disk:

- Every entry's path is validated against path traversal: an absolute path, a Windows drive-letter or UNC prefix,
  or a `..` component anywhere in the path is rejected — checked against a backslash-normalized copy of the name
  so a Windows-style `..\..\evil.txt` is caught the same as a Unix-style `../../evil.txt`.
- Symlink entries are rejected outright. Helldivers 2 mods never need them, and a symlink entry is a classic way
  to smuggle a write outside the destination folder.
- After extraction, DDMM re-walks what actually landed on disk (without following any symlinks it finds there)
  and confirms every file's fully resolved path is still inside the mod's own folder — a backstop for archive
  formats whose extraction routines don't sanitize paths themselves.

If any check fails, the whole archive is rejected and nothing from it is kept.

DDMM's `.7z` library dependency (`sevenz-rust2`) had its own path-traversal issue, tracked as
[CVE-2026-61725 / GHSA-qh76-45cr-8xrc](https://github.com/advisories/GHSA-qh76-45cr-8xrc); the dependency has been
upgraded to a version with the fix. It's worth noting DDMM's own entry-path checks above already caught and
rejected this exact kind of malicious `.7z` archive before the fix landed — this upgrade removes a second point
of exposure, it wasn't the only thing standing in the way.

## Downloads are `https://` only, size-capped, and content-checked

[Add URL](../using/adding-mods.md#add-url) only accepts `https://` links — plain `http://` and any other scheme
(`file:`, `javascript:`, etc.) are refused outright. Downloads are capped at 2 GiB, enforced both from the
server's reported size and by counting bytes as they stream in. Once downloaded, the file's actual content is
checked against known zip/7z/rar file signatures (not just its name or URL) before it's treated as a mod archive
— so a site that serves back an HTML login page instead of a file is rejected rather than "installed" as garbage.

## Links opened in your browser

A source's "Open on &lt;site&gt;" page link — whether declared by a mod's manifest, or recorded by DDMM when you
add a mod from a URL — is only ever opened if it's a plain `http://` or `https://` URL. Anything else a manifest
might contain (`javascript:`, `file:`, a custom scheme) is silently dropped and never handed to your OS's URL
opener.

## Filesystem safety when adding a local folder

Installing a mod from a plain folder (["Add Folder"](../using/adding-mods.md#add-folder) or drag & drop) copies
files in; it never follows symlinks found inside the source folder, so a symlink pointing outside it can't be used
to pull in files you didn't intend to share.

## The browser handoff only watches, never reads

During a [browser handoff](../using/mod-sites.md#how-the-browser-handoff-works), DDMM polls your configured
Downloads folder for a new file to appear — it never opens, uploads, or inspects the contents of anything else
already in that folder, only file names and sizes. A candidate that turns out to be a symlink is refused, the
same as any other archive install.

## Browser bridge & deep links

The [browser extension](../using/one-click-install.md) talks to DDMM over a TCP connection bound to
`127.0.0.1` only — never reachable from the network — on a random port chosen at startup. A fresh 256-bit token,
regenerated every launch, is required before DDMM accepts anything on that connection; it's compared in constant
time to avoid leaking timing information, and it's written (alongside the port) to `bridge.json` in DDMM's data
folder with owner-only file permissions, deleted again on clean exit.

Only the two DDMM browser extensions — identified by extension ID, never by anything a web page could spoof — are
accepted callers; a message from anything else is rejected outright. Message sizes are capped at 1 MB each way,
and an install still goes through every one of the archive checks above (path traversal, symlinks, magic bytes,
the 2 GiB size cap) — the bridge changes *how* a file arrives, not what DDMM is willing to do with it once it
does. The first time a given site asks to install through the extension, DDMM always asks permission first
(see [Settings → Sites Allowed to Install Through the Extension](../using/settings.md#sites-allowed-to-install-through-the-extension));
nothing installs silently.

`ddmm://` links work differently: **any** web page can trigger one, so DDMM always shows a confirmation — with no
"always allow" — and only ever accepts an `https://` target; anything else in the link is ignored. See
[One-click install](../using/one-click-install.md#ddmminstall-links) for what the user sees.

As with every other install path, no credentials, cookies, or API keys for any mod site ever cross the bridge (the
optional Nexus key included) —
the browser extension downloads using your own logged-in session, the same as the
[browser handoff](../using/mod-sites.md#how-the-browser-handoff-works) does.

## Update checks are opt-in and size-capped

Update checks never run unless you ask: when you click [Check for Updates](../using/updating-mods.md), or, if you
turned it on in Settings (off by default), when DDMM starts and optionally every N hours while it's open. Each
request (an AyakaMods mod page, or the GitHub, GameBanana, ModWorkshop or Nexus Mods APIs) is `https://` only,
times out, is spaced out per site, and is capped at 5 MB, the same belt-and-braces size-limit approach used for
archive downloads.

One-click updates only start a download from the site's own hosts (GitHub, GameBanana and ModWorkshop download
hosts), and the file then goes through the same archive checks as every other install.
