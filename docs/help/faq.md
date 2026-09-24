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

Check whether the mod uses a `V2` manifest. DDMM can display `V2` options, but deploying a `V2`-manifest mod isn't
implemented yet — see [Mod options & variants](../using/options-variants.md#v1-v2-manifests-toggle-sub-options).
A `V1` or Legacy manifest deploys normally.

### My game shows a "files may be corrupt" / fatal error after a game update

This generally means one of your deployed mods hasn't been updated for the game's latest patch — the mod's patch
files no longer match what the game expects, not a problem DDMM itself caused. Purge, then re-deploy only mods you
know are up to date for the current game version; see [Deploy & purge](../using/deploy-purge.md).

### "Add URL" failed

The link has to serve the archive file directly. A mod **page** (Nexus Mods, AyakaMods, etc.) generally isn't a
direct link — see [Mod sites](../using/mod-sites.md) for how those sites are meant to be added instead, or
download the archive yourself and add it with [Add](../using/adding-mods.md#add-archive-files).

### Where do I ask questions or get help that isn't a bug report?

See [Reporting bugs](bugs.md) for the distinction, and the project's
[GitHub Issues](https://github.com/katsyk/DemocracyDefenderModManager/issues) and
[repository](https://github.com/katsyk/DemocracyDefenderModManager) for where the project is discussed.
