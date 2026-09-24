---
title: Checking for updates
---

# Checking for updates

!!! info "Landing feature"
    Update checking is landing alongside this documentation. The Library panel's update button already exists in
    the interface, but is currently disabled/not wired up in this codebase. The behavior below describes the
    intended design — check the
    [release notes](https://github.com/katsyk/DemocracyDefenderModManager/releases) for what's actually shipped
    in your version.

DDMM only checks for mod updates when you ask it to — never automatically in the background.

## Checking

Click **Check for updates** to compare your installed mods against their source site. Two sources are supported:

- **AyakaMods** — reads the mod page's public version information (no login needed for this, unlike downloading).
- **GitHub Releases** — reads the repository's latest release.

**Nexus Mods is not supported** for update checks, because doing so would require a personal Nexus API key, which
DDMM does not ask you to provide.

## Updating a mod

A mod with an available update gets a badge in your Library. Its update button now reads **"Update from
&lt;site&gt;"** and starts the same [browser handoff](mod-sites.md#how-the-browser-handoff-works) used for adding
a mod from that site — once the new archive is picked up, it replaces the mod's files in place while keeping your
existing profile settings (enabled state, options, position) for it.

## Mods DDMM can't check

A mod only gets update badges if DDMM knows a supported source for it — see
[The Sources field](../authors/sources.md) for how a mod declares where it came from. A mod with no `Sources`
entry, or one sourced only from Nexus Mods or an unsupported site, simply has nothing to check against.
