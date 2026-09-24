---
title: FAQ
---

# FAQ

### Do I need an account on Nexus Mods, AyakaMods, or any other site?

No. DDMM never asks for, stores, or uses credentials for any mod site. Sites that require being logged in to
download hand the actual download off to your own browser session — see [Mod sites](../using/mod-sites.md).

### Where does DDMM store my mods and settings?

Everything lives next to the DDMM executable: mods in `mods/`, settings in `settings.json`, profiles in
`profiles.json`. See [Download & install](../getting-started/download.md#where-things-are-stored).

### Can I mix mods from different sites in one profile?

Yes — DDMM doesn't care where a mod came from. A profile can freely mix mods installed from AyakaMods, Nexus Mods,
GitHub, a direct link, and a local folder.

### I installed a mod with options, but nothing happens when I deploy it

Check that at least one option is toggled on in the options editor (pencil button) — an option with no `Include`
folders of its own, and no sub-option selected, contributes nothing. See
[Mod options & variants](../using/options-variants.md#v1-v2-manifests-toggle-sub-options); this applies the same
way to `V1` and `V2` manifests.

### My game shows a "files may be corrupt" / fatal error after a game update

This generally means one of your deployed mods hasn't been updated for the game's latest patch — the mod's patch
files no longer match what the game expects, not a problem DDMM itself caused. Purge, then re-deploy only mods you
know are up to date for the current game version; see [Deploy & purge](../using/deploy-purge.md).

### "Add URL" failed

An AyakaMods or Nexus Mods **mod page** link shouldn't hit this — DDMM recognizes both and goes straight to the
[browser handoff](../using/mod-sites.md#how-the-browser-handoff-works) instead of attempting a direct download.
For any other site, this means the link didn't serve a downloadable archive directly (and either you weren't
offered the "Open in Browser Instead?" handoff, or you declined it, or the handoff itself failed/timed out too).
Download the archive yourself and add it with [Add](../using/adding-mods.md#add-archive-files), or try
[Add URL](../using/adding-mods.md#add-url) again.

### The browser handoff is stuck on "Waiting for the download..."

Make sure your [Downloads Folder](../using/settings.md#downloads-folder) setting actually points at where your
browser saves files — the handoff only notices a file that lands in that exact folder, ignores files it considers
still in progress, and gives up after 15 minutes. If you already have the file (saved somewhere else, or from
before the handoff started), use **"I already downloaded it -- choose file"** instead of waiting.

### "Check for Updates" doesn't show a badge for a mod I know has a new version

Update checks only cover mods with a recognized, [supported](../using/updates.md#which-sites-are-supported)
source (AyakaMods or GitHub) *and* a recorded installed version to compare against — see
[How DDMM knows a mod's installed version](../using/updates.md#how-ddmm-knows-a-mods-installed-version). A mod
installed manually, with no `Sources` declared, or sourced only from Nexus Mods or an unsupported site, has
nothing for DDMM to check.

### Where do I ask questions or get help that isn't a bug report?

See [Reporting bugs](bugs.md) for the distinction, and the project's
[GitHub Issues](https://github.com/katsyk/DemocracyDefenderModManager/issues) and
[repository](https://github.com/katsyk/DemocracyDefenderModManager) for where the project is discussed.
