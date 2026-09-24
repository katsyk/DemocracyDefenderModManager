---
title: Settings reference
---

# Settings reference

The Settings page has three actual settings, stored in `settings.json` in DDMM's
[data folder](../getting-started/download.md#data-location), plus a read-only line showing where that data
folder is.

## Game Path

The folder your Helldivers 2 install lives in — see [First-time setup](../getting-started/setup.md) for the
validation rules, error messages, and how automatic detection works. This is required: DDMM won't let you leave
the Settings page, and refuses to [deploy or purge](deploy-purge.md), until it's valid.

- **Label:** Game Path
- **Placeholder:** `eg. Steam/steamapps/common/Helldivers 2/`
- **Auto-detect button:** looks for Helldivers 2 via Steam (the same check DDMM runs automatically on startup
  when the path is invalid) and fills in the field if it finds a valid install
- **Browse... button:** opens a folder picker

## Downloads Folder

The folder DDMM watches during a [browser handoff](mod-sites.md#how-the-browser-handoff-works) — its own
description reads: "Where DDMM watches for a mod you download from a site it can't fetch directly (e.g. AyakaMods,
Nexus Mods)." Defaults to your OS's normal Downloads folder.

- **Label:** Downloads Folder
- **Placeholder:** `eg. Downloads`
- **Browse... button:** opens a folder picker

Unlike Game Path, this field isn't validated on the Settings page itself — an invalid or missing folder only
surfaces as an error at the moment you actually start a handoff ("the downloads folder isn't set or doesn't
exist -- check it in Settings"). Only the browser handoff uses this setting; every other install method (Add,
Add Folder, Add URL for a direct link) is unaffected by it.

## Skip List

A list of 16-character lowercase hex patch-name prefixes (for example `0cf14e223de06a26`) that already occupy
`.patch_0` in your `data` folder — typically a DLC's own patch file. When a patch name you're deploying is in this
list, DDMM starts numbering its files at `.patch_1` instead of `.patch_0`, so a mod touching the same asset
doesn't collide with — or overwrite the slot used by — that DLC content. See
[Deploy & purge](deploy-purge.md#the-skip-list-and-patch-numbering).

- **Label:** Skip List
- Add an entry with the **+** button: a popup asks for exactly 16 hexadecimal characters
  (placeholder: `eg. 0cf14e223de06a26`).
- Select an entry in the list and use the **-** button to remove it.

You won't normally need to touch this unless a specific mod's documentation tells you to add an entry for it.

## Data Folder

Read-only — shows the folder DDMM is currently keeping `mods/`, `settings.json`, `profiles.json` and its logs in.
Which folder that actually is depends on how you installed DDMM; see
[Data location](../getting-started/download.md#data-location) for the full rules.

- **Label:** Data Folder
- **Open Folder button:** opens that folder in your system's file manager

## Where settings live

Settings are saved to `settings.json` in DDMM's data folder (see **Data Folder** above), in a versioned format
(currently `V1`). You generally shouldn't need to hand-edit this file — use the Settings page instead.
