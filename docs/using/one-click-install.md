---
title: One-click install (browser extension)
---

# One-click install (browser extension)

The DDMM browser extension adds an **Install with DDMM** button to mod pages, so you never have to download a
file and then separately open DDMM to add it — including on sites that require being logged in, like Nexus Mods
and AyakaMods. It talks to DDMM over a private, local-only connection; the full technical contract is in
[Browser bridge protocol](../development/bridge-protocol.md) if you want the details.

## Install the browser extension

The DDMM browser extension adds an **Install with DDMM** button to mod pages on AyakaMods, Nexus
Mods, ModWorkshop, GameBanana and GitHub, plus a right-click **Install with DDMM** on any download
link, on any site. It never collects any data -- see the extension's
[privacy policy](https://github.com/katsyk/DemocracyDefenderModManager/blob/main/extension/PRIVACY.md)
for the details.

### Chrome, Edge, Brave

The extension isn't in the Chrome Web Store yet. Until it is:

1. Download `ddmm-extension-chrome-<version>.zip` from the [latest release](https://github.com/katsyk/DemocracyDefenderModManager/releases).
2. Unzip it somewhere you'll keep it (don't delete the folder afterwards -- the browser loads the
   extension from it directly).
3. Open `chrome://extensions` (or `edge://extensions`, `brave://extensions`).
4. Turn on **Developer mode** (top right).
5. Click **Load unpacked** and select the unzipped folder.

The extension icon appears in your toolbar. Click it any time to check DDMM's connection status.

### Firefox

The extension isn't signed by Mozilla (AMO) yet, so for now it can only be loaded **temporarily**
-- it disappears when Firefox restarts, and you'll need to reload it each session. Permanent
installation needs the AMO-signed version, which is coming soon.

1. Download `ddmm-extension-firefox-<version>.zip` from the [latest release](https://github.com/katsyk/DemocracyDefenderModManager/releases) and unzip it.
2. Open `about:debugging#/runtime/this-firefox`.
3. Click **Load Temporary Add-on...** and select the `manifest.json` inside the unzipped folder.

### Using it

- **The button.** On a mod page, look for the **Install with DDMM** button (near the site's own
  download button where possible; otherwise a small yellow-and-black button in the bottom corner of
  the page). Click it and DDMM installs the mod. If the site needs you to click its own download
  control first (a login-gated file, or Nexus's "Slow download"), the button tells you to -- DDMM
  picks up the download automatically once it finishes.
- **Right-click any download link**, on any site, and choose **Install with DDMM** to send that
  file straight to DDMM.
- **Auto-capture.** Open the extension popup and turn on auto-capture for a site to have every
  archive you download from it install automatically, with no button click at all. It's off by
  default everywhere.

### Button states

| Label | Meaning |
| --- | --- |
| Install with DDMM | Not installed yet -- click to install. With the hint "DDMM will start", DDMM is closed: clicking starts it and installs in one go |
| Starting DDMM... | DDMM is starting up for the install you clicked |
| Installing... | Working on it |
| Installed (check mark) | Already installed -- click to reinstall |
| Update with DDMM | A newer version is available -- click to update |
| Get DDMM | The extension can't find DDMM on this computer -- click for setup help |

Just visiting a mod page never starts DDMM. Only clicking the button (or **Start DDMM** in the
extension's popup) does.

### Troubleshooting

**"DDMM not found" / the button always says "Get DDMM":** Open DDMM at least once so it can
register itself with your browser, then in DDMM go to **Settings -> Browser integration -> Repair**.
"Get DDMM" means the browser couldn't find DDMM's browser integration at all. A DDMM that's
installed but closed shows "Install with DDMM" with "DDMM will start" instead.

**The button never appears on a mod page:** Make sure you're on an actual mod page (not a search or
listing page), and that the extension is enabled for that site in your browser's extension settings.
As a fallback, right-click the mod's download link and choose **Install with DDMM**.

**A download never gets picked up:** Capture only lasts 10 minutes after you click the button, and
only recognizes archive files (`.zip`, `.7z`, `.rar`) from that site (including its known CDN
domains). If your download is a different format, use the right-click **Install with DDMM** on the
link, or add the file manually in DDMM.

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
