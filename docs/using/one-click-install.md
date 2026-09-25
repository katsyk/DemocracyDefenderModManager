---
title: One-click install (browser extension)
---

# One-click install (browser extension)

The DDMM browser extension adds an **Install with DDMM** button to mod pages, so you never have to download a
file and then separately open DDMM to add it — including on sites that require being logged in, like Nexus Mods
and AyakaMods. It talks to DDMM over a private, local-only connection; the full technical contract is in
[Browser bridge protocol](../development/bridge-protocol.md) if you want the details.

<!-- EXTENSION-INSTALL -->
## Install the browser extension

<!-- Extension-side install instructions (store links, browsers supported, screenshots) go here. -->

## How it works

1. You click **Install with DDMM** (or **Update with DDMM**, if a newer version is available) on a supported mod
   page.
2. The extension downloads the file using *your own, already-logged-in browser session* — DDMM never asks for,
   stores, or sees your login for any site.
3. Once the download finishes, the extension hands the file to DDMM through a native-messaging connection local
   to your machine.
4. **The first time** a site does this, DDMM asks:

   > **Allow Browser Install?** — Allow the DDMM browser extension to install mods from &lt;site&gt;?

   with **Always Allow**, **Just This Once**, and **Deny**. Choosing **Always Allow** skips this prompt for that
   site from then on — revocable any time in
   [Settings → Sites Allowed to Install Through the Extension](settings.md#sites-allowed-to-install-through-the-extension).
5. DDMM installs the mod exactly like [Add URL](adding-mods.md#add-url) would — if a mod from the same site is
   already installed, it's updated in place instead of duplicated.
6. Depending on [Settings → After a Browser Install](settings.md#after-a-browser-install), DDMM then adds the mod
   to your library only, adds it to your active profile, or adds it and deploys immediately.
7. A small notification in the corner of the window reports what happened — installed, added to a profile,
   deployed, or (rarely) installed but with something to look at, like Helldivers 2 currently running.

## `ddmm://install` links

Any web page — a mod author's own site, a Discord message, a forum post — can link directly to an install without
needing the extension at all:

```
ddmm://install?url=https%3A%2F%2Fexample.com%2Fmods%2Fcool-mod.zip
```

Clicking one always shows a confirmation first ("A link wants to install a mod from &lt;site&gt;. Install?") —
unlike the extension, there's no "always allow" for these, since any page could contain one. Only `https://`
links are accepted; anything else is silently ignored. See
[For mod authors → Install button for your mod page](../authors/install-button.md) if you want to add one of
these to your own mod's page.

## Auto-import from Downloads

A separate, **opt-in, off by default** option: turn on
[Settings → Auto-import from Downloads](settings.md#auto-import-from-downloads) and DDMM watches your Downloads
folder the whole time it's running (not just during a [browser handoff](mod-sites.md#how-the-browser-handoff-works))
for anything that looks like a Helldivers 2 mod archive — useful for sites the extension doesn't have a button on
yet. It never installs anything without you clicking **Install** (or **Install & Deploy**) on the notification
first.

## Browser integration

DDMM registers itself with your installed browsers automatically, every time it starts, so the extension can find
it. If a browser stops recognizing DDMM (for example, after moving a portable install to a new folder), use
**Repair** next to that browser in
[Settings → Browser Integration](settings.md#browser-integration) — no reinstall needed.
