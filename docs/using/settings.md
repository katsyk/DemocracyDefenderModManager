---
title: Settings reference
---

# Settings reference

The Settings page currently has two settings, stored in `settings.json` next to the DDMM executable.

## Game Path

The folder your Helldivers 2 install lives in — see [First-time setup](../getting-started/setup.md) for the
validation rules and error messages. This is required: DDMM won't let you leave the Settings page, and refuses to
[deploy or purge](deploy-purge.md), until it's valid.

- **Label:** Game Path
- **Placeholder:** `eg. Steam/steamapps/common/Helldivers 2/`
- **Browse... button:** opens a folder picker

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

## Downloads folder

!!! info "Landing feature"
    A setting for the folder DDMM watches during the [browser handoff](mod-sites.md#how-the-browser-handoff-works)
    (defaulting to your OS's normal Downloads folder) is part of the AyakaMods/Nexus Mods support landing
    alongside this documentation, and isn't present in this codebase's Settings page yet.

## Where settings live

Settings are saved to `settings.json` in the same folder as the DDMM executable, in a versioned format (currently
`V1`). You generally shouldn't need to hand-edit this file — use the Settings page instead.
