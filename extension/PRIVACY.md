# DDMM browser extension -- privacy policy

Short version: the DDMM browser extension collects nothing, sends nothing anywhere on its own, and
everything it touches stays on your computer.

## What the extension does

- Adds an "Install with DDMM" button to mod pages on AyakaMods, Nexus Mods, ModWorkshop, GameBanana
  and GitHub, and a right-click "Install with DDMM" item on any download link on any site.
- When you click it, the extension downloads the mod file using your browser's own `downloads` API
  (with your existing login session on that site, if any), then hands the downloaded file's local
  path to DDMM over a native-messaging connection on your own machine.
- Talks only to `io.github.katsyk.ddmm`, the native messaging host that DDMM itself registers when
  installed. That connection never leaves your computer -- it's local interprocess communication, not
  a network request.

## What it does not do

- **No data collection, no analytics, no telemetry.** The extension has no server of its own and
  contacts no server of ours. There is nothing to opt out of because nothing is sent in the first
  place.
- **No remote code.** Every script the extension runs ships inside the extension package itself and
  is reviewed as part of DDMM's normal release process; nothing is fetched and executed at runtime.
- **No credentials, cookies, or tokens ever cross the bridge to DDMM.** The browser does the
  downloading with your own session; DDMM only ever receives a finished file's path.
- **No page content is read beyond what's needed to work the button**: the mod page's URL, and, on
  supported sites, a version string the page already displays (e.g. AyakaMods' own version field).
  The extension does not read page text, forms, or anything else, and it never reads or transmits
  content from a page that isn't a recognized mod page.
- **No host permissions beyond the five known mod sites** (AyakaMods, Nexus Mods, ModWorkshop,
  GameBanana, GitHub). On every other site, the extension only ever acts through the right-click
  context menu on a link you choose, or the `downloads` events for a file you (or a context-menu
  click) started -- it has no standing access to those pages' content.

## What DDMM stores locally

The extension keeps a small amount of state in your browser's local `storage.local`, never
synced or uploaded anywhere:

- Your per-site "auto-capture" toggle (off by default for every site).
- Your "after install" preference override, if you've set one.
- A list of your last 10 installs (mod name, source, version), so the popup can show you what just
  happened.

Uninstalling the extension removes all of this.

## Button clicks and trust

The install button only ever responds to real, trusted clicks -- a mod site's own page script cannot
click it for you (the extension checks `event.isTrusted` on every click before doing anything).

## Questions

This extension is part of DDMM, an open-source project. Its full source is in this repository under
`extension/`; the native messaging protocol it speaks to DDMM is documented at
`docs/development/bridge-protocol.md`.
