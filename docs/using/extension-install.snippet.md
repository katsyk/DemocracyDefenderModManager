<!-- Written for docs/using/one-click-install.md's <!-- EXTENSION-INSTALL --> section. -->

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
| Install with DDMM | Not installed yet -- click to install |
| Installing... | Working on it |
| Installed (check mark) | Already installed -- click to reinstall |
| Update with DDMM | A newer version is available -- click to update |
| Get DDMM | DDMM isn't reachable -- click for setup help |

### Troubleshooting

**"DDMM not found" / the button always says "Get DDMM":** Open DDMM at least once so it can
register itself with your browser, then in DDMM go to **Settings -> Browser integration -> Repair**.
If that doesn't fix it, make sure DDMM is actually running (the extension talks to a running copy of
DDMM, not just the installed app) and try again.

**The button never appears on a mod page:** Make sure you're on an actual mod page (not a search or
listing page), and that the extension is enabled for that site in your browser's extension settings.
As a fallback, right-click the mod's download link and choose **Install with DDMM**.

**A download never gets picked up:** Capture only lasts 10 minutes after you click the button, and
only recognizes archive files (`.zip`, `.7z`, `.rar`) from that site (including its known CDN
domains). If your download is a different format, use the right-click **Install with DDMM** on the
link, or add the file manually in DDMM.
