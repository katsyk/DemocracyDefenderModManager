---
title: Mod sites
---

# Mod sites

DDMM doesn't privilege any one mod site. A mod's origin is just a `Provider` string (see
[The Sources field](../authors/sources.md)) — well-known ones get a nice display name and an automatic page-URL
template, anything else still works with an explicit link.

Paste anything into **Add URL** (tip: "Add a mod from a direct download link.") — its popup description reads:

> Paste a mod page or download link from AyakaMods, Nexus Mods, ModWorkshop, GameBanana, GitHub, or any direct
> download link.

What happens next depends on the link:

- **AyakaMods or Nexus Mods mod-page links** always go straight to the **browser handoff** below — DDMM never
  even attempts to download these itself, since both sites gate the actual file behind being logged in.
- **Any other link** (a direct file link, a ModWorkshop/GameBanana/GitHub page, a personal file host, ...) —
  DDMM tries downloading it directly first. If what comes back is HTML instead of an archive (a login wall, a
  page with a JavaScript download button), DDMM asks:

  > **Open in Browser Instead?** — That link didn't serve a downloadable archive directly. Open it on &lt;site&gt;
  > in your browser and let DDMM install it automatically once it downloads?

  Confirming starts the same browser handoff.

DDMM never asks for, stores, or uses credentials for any mod site — every handoff downloads through your own,
already-logged-in browser session.

## How the browser handoff works

1. DDMM opens the mod's page in your default browser.
2. A **"Browser Handoff"** popup appears: "Download the mod in the browser tab that just opened for &lt;site&gt;.
   DDMM installs it automatically as soon as it lands in &lt;your Downloads folder&gt;." with a spinner reading
   "Waiting for the download...".
3. You click download on the site as normal, logged in as yourself.
4. DDMM polls your [Downloads folder](settings.md#downloads-folder) once a second for a new archive: it ignores
   files that already existed before the handoff started, ignores browser in-progress names
   (`.crdownload`, `.part`, `.tmp`, `.download`, `.partial`), and ignores anything that isn't a `.zip`/`.7z`/`.rar`
   file (or is a symlink). Once a candidate appears, DDMM waits for its file size to stop changing for two
   consecutive checks before treating the download as finished, then installs it — the popup switches to
   "Installing...".
5. If nothing matching shows up within **15 minutes**, the handoff gives up and tells you so.
6. At any point you can press **Cancel**, or **"I already downloaded it -- choose file"** to pick a file yourself
   instead of waiting for the watch to find one.

Only one handoff can run at a time. Whichever source the link resolves to (see
[The Sources field](../authors/sources.md)) is recorded the same way a direct-URL install is, so the mod still
gets an "Open on &lt;site&gt;" menu entry afterward.

## AyakaMods

Made with love for the AyakaMods community ❤️ — one of the best homes for Helldivers 2 modding, and DDMM's home
turf. Go say hi: [ayakamods.com/games/helldivers-2](https://ayakamods.com/games/helldivers-2.119/).

[AyakaMods](https://ayakamods.com) is a first-class, built-in `Provider` in DDMM: its page-URL shape
(`https://ayakamods.com/mods/<slug>.<id>/` or `https://ayakamods.com/mods/<id>/`) is recognized directly, mod
pages always use the browser handoff above, and [Check for updates](updates.md) reads the same page's published
version automatically. At install time, DDMM also fetches the mod page once to record its current version for
future update checks — see [Checking for updates](updates.md#how-ddmm-knows-a-mods-installed-version).

## Nexus Mods

Nexus Mods gates its downloads behind a login too, so a Nexus Mods mod-page link always uses the browser handoff.
A direct Nexus **file** link (not the mod page) can still install immediately, if you have one.

Update checking against Nexus Mods is **not** supported — it would require a personal Nexus API key DDMM doesn't
ask you to provide. See [Checking for updates](updates.md).

## ModWorkshop, GameBanana

Both are supported `Provider` values with an automatic page-URL template (see
[The Sources field](../authors/sources.md)), so a mod declaring one gets an "Open on ModWorkshop" / "Open on
GameBanana" menu entry. Pasting a link from either isn't hardcoded to the browser handoff the way AyakaMods and
Nexus Mods are: DDMM tries a direct download first, and only offers the browser handoff if that comes back as an
HTML page rather than an archive.

DDMM doesn't have an update-check integration for either site.

## GitHub

GitHub is a supported `Provider` too, and its release-asset links are ordinarily plain `https://` links that don't
require a login, so they install immediately through **Add URL**. [Check for updates](updates.md) also supports
GitHub, reading a repository's latest release tag.

## Other / unrecognized sites

Any other link is still handled sensibly: DDMM tries a direct download, and if the response turns out to be HTML
instead of an archive, it offers the same "Open in Browser Instead?" handoff rather than just failing.

A mod's manifest can also declare any custom `Provider` string it wants (see
[The Sources field](../authors/sources.md)); DDMM shows it as-is in the "Open on &lt;site&gt;" menu using
whatever page URL the manifest gives it, though only the sites listed above get handoff/update-check integration.
