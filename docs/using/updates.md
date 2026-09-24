---
title: Checking for updates
---

# Checking for updates

DDMM only checks for mod updates when you click **Check for Updates** (tip: "Check installed mods against their
source site for a newer version.") — a button on the Mods page, alongside Add / Add Folder / Add URL. There is no
background polling and no check at startup.

## Which sites are supported

| Site | Support | How |
| --- | --- | --- |
| AyakaMods | Supported | Reads the `softwareVersion` field from the mod page's own structured (JSON-LD) metadata |
| GitHub | Supported | Reads the repository's latest release (`tag_name`) via the public GitHub API |
| Nexus Mods | **Unsupported** | Would require a personal Nexus API key, which DDMM doesn't collect |
| Anything else | Not checked | Mods sourced only from another site (or with no `Sources` at all) are skipped entirely — not reported as "unsupported", just not checked |

A single mod can have more than one source; DDMM checks every AyakaMods/GitHub/Nexus source it has, independently.

## Seeing the results

After a check, a popup reports either "*N* update(s) available." or "Everything is up to date." Per mod:

- A mod with an update gets a small cloud-download badge next to its name (tooltip: "Update available"), in both
  the profile list and the Library.
- Its context menu (three-dot button, only visible in your active profile's list) gains an
  **"Update from &lt;site&gt;"** entry per updatable source — a mod could in principle have more than one.
- In the Library panel, that mod's update button (previously always disabled) becomes clickable; it updates from
  whichever updatable source it finds first.

!!! note "Only "update available" shows in the UI"
    A check can also come back "up to date", "unknown" (not enough information — see below), "unsupported"
    (Nexus), or "error" (the request itself failed) for a given mod/source — but none of those states are shown
    anywhere per-mod today. The only visible signal is the badge/menu entry when an update **is** available, plus
    the aggregate count/"up to date" popup after running the check. If a mod you expected a badge on doesn't have
    one, re-running the check is currently the only way to see why.

## Updating a mod

Clicking **"Update from &lt;site&gt;"** (menu) or the Library's update button starts the same
[browser handoff](mod-sites.md#how-the-browser-handoff-works) used for adding a mod from that site. Once the new
archive is found and installed, it replaces the mod's files in place.

**GUID handling:** if the update's own archive ships no `manifest.json` (DDMM generates one, same as any
manifest-less install), the mod keeps its **original** GUID, so your profile's enabled state, options, and
position for it survive untouched. If the update *does* ship its own manifest with its own `Guid`, that new GUID
wins instead — even if it differs from the old one — and profile entries referencing the old GUID stop matching
it (DDMM doesn't try to migrate them).

## How DDMM knows a mod's installed version

An update check compares each source's recorded `Version` against the site's current version — if there's nothing
recorded, the result is "unknown" rather than a guess. The installed version comes from, in order:

1. `Version` on the matching entry in the mod's manifest `Sources` array (see
   [The Sources field](../authors/sources.md)), if the mod author set one.
2. Otherwise, whatever DDMM itself recorded in the `.hd2mm-origin.json` sidecar at install time.

For **AyakaMods** specifically, DDMM fetches the mod page once at install time (whether installed via the browser
handoff or "choose file") and stamps its published `softwareVersion` into the sidecar automatically — so a mod
installed through DDMM from AyakaMods has a working update check from day one with no extra effort. This is
best-effort: if the fetch fails for any reason, the install still succeeds, just without a recorded version (so
the first update check for it will read "unknown" until a version is available some other way).

**For mod authors:** if you declare an `ayakamods` source in your own manifest's `Sources` field, set its
`Version` to match the value AyakaMods publishes for your mod — that's what makes update checks work for anyone
who installs your mod with its manifest already declaring the source, not just installs made through the handoff.
See [The Sources field](../authors/sources.md#ayakamods).
