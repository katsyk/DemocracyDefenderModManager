---
title: Mod sites
---

# Mod sites

DDMM doesn't privilege any one mod site. A mod's origin is just a `Provider` string (see
[The Sources field](../authors/sources.md)) — well-known ones get a nice display name and an automatic page-URL
template, anything else still works with an explicit link.

How you actually get a mod **into** DDMM depends on whether the site lets you download without logging in:

- **Direct download links** (a plain `https://` link to the archive itself — GitHub release assets, a personal
  file host, etc.) install immediately through [Add URL](adding-mods.md#add-url): DDMM downloads and installs the
  file itself, no browser involved.
- **Sites that require being logged in to download** (AyakaMods, Nexus Mods) can't be downloaded by DDMM directly,
  since DDMM never asks for, stores, or uses your credentials on any site. Instead, DDMM uses a **browser
  handoff**: it opens the mod's page in your normal browser, you log in and click download there like you always
  have, and DDMM picks the finished download up automatically.

## How the browser handoff works

!!! info "Landing feature"
    The browser handoff described on this page is part of AyakaMods/Nexus Mods support that's landing alongside
    this documentation. If your DDMM build doesn't have it yet, download the mod through your browser as usual
    and add the resulting archive with [Add](adding-mods.md#add-archive-files) or drag & drop.

1. Paste the mod's page link into **Add URL**.
2. DDMM recognizes it as a site that needs a login, and opens that page in your default browser instead of trying
   to download it itself.
3. You click download on the site as normal.
4. DDMM watches your **Downloads folder** (configurable in [Settings](settings.md#downloads-folder), default: your
   OS's normal Downloads folder) for a new archive to appear, and installs it automatically once it does —
   remembering which mod and version it came from, the same as any other source.
5. While DDMM is waiting, you can cancel, or use **"I already downloaded it — choose file"** to point it at a file
   you already have instead of waiting for a new download.

The same mechanism is used for any site that needs a login to download, not just AyakaMods and Nexus Mods.

## AyakaMods

[AyakaMods](https://ayakamods.com) is a first-class site in DDMM. Paste a mod page link from ayakamods.com into
**Add URL** and DDMM walks you through the browser handoff above. DDMM never asks for your AyakaMods credentials —
downloading always happens in your own logged-in browser session.

## Nexus Mods

Nexus Mods gates its downloads behind a login too, so pasting a Nexus Mods mod page link into **Add URL** uses the
same browser handoff. A direct Nexus **file** link (not the mod page) can still be installed immediately like any
other direct link, if you have one.

Update checking against Nexus Mods specifically is **not** supported, because it requires a personal Nexus API
key DDMM doesn't ask you to provide — see [Checking for updates](updates.md).

## ModWorkshop, GameBanana

Both are supported `Provider` values with an automatic page-URL template (see
[The Sources field](../authors/sources.md)), so a mod declaring a ModWorkshop or GameBanana source gets an
"Open on ModWorkshop" / "Open on GameBanana" menu entry. Whether downloading from these sites requires a login
depends on the site and the specific mod; if a direct file link works, [Add URL](adding-mods.md#add-url) installs
it immediately, otherwise use the browser handoff (or download manually and add the archive).

## GitHub

GitHub is a supported `Provider` too. GitHub release assets are usually plain `https://` links that don't require
a login, so they install immediately through **Add URL** without any browser handoff.

## Other / unrecognized sites

Pasting a link from a site DDMM doesn't specifically recognize is still handled sensibly:

- If it's a direct link to an archive, it installs immediately.
- If it turns out to serve an HTML page instead (a login wall, a page with a JavaScript download button), the
  install fails with a hint to use a direct link or add the file manually — see [Add URL](adding-mods.md#add-url).

A mod's manifest can also declare any custom `Provider` string it wants (see
[The Sources field](../authors/sources.md)); DDMM shows it as-is in the "Open on &lt;site&gt;" menu using
whatever page URL the manifest gives it.
