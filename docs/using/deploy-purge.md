---
title: Deploy & purge
---

# Deploy & purge

Helldivers 2 loads mods as **patch files** dropped directly into its `data` folder — files named
`<16 hex characters>.patch_<N>`, each optionally paired with a `.gpu_resources` and/or `.stream` file of the same
name. DDMM's Deploy and Purge buttons manage exactly those files; nothing else in your game install is touched.

## Deploy

Clicking **Deploy** (tip: "Install the current selection of mods."):

1. Validates your [Game Path](settings.md#game-path) — deploy refuses to run against an invalid path.
2. **Purges first** (see below), so every deploy starts from a clean `data` folder rather than layering on top of
   a previous one.
3. Walks your active profile's mod list, skipping any mod whose toggle is switched off.
4. For each enabled mod, collects its patch files — from the mod's root, from the selected legacy option's
   subfolder, or from each toggled `V1` option's (and selected sub-option's) `Include` folders, depending on its
   [manifest](../authors/packaging.md#folder-layout-by-manifest-type) — grouped by their 16-character patch name.
5. Copies each group's patch/`.gpu_resources`/`.stream` files into `<Game Path>/data/`, numbering them
   `.patch_0`, `.patch_1`, and so on in your profile's mod order. If a triplet is missing its `.gpu_resources` or
   `.stream` file, DDMM writes an empty placeholder for it instead of skipping it, so the numbering for later
   mods touching the same patch name stays consistent.

Deploying an empty profile (no mods, or none enabled — same list of `Configs`) shows an error instead of purging
your game folder for nothing: "Can not deploy empty profile!"

!!! danger "V2 manifests can't be deployed yet"
    See [Mod options & variants](options-variants.md#v1-v2-manifests-toggle-sub-options) — a `V2`-manifest mod
    included (and enabled) in the profile you deploy will currently fail deployment rather than install its
    files, because `V2` deploy support isn't implemented yet.

### The Skip List and patch numbering

Some patch-name prefixes are already used by the base game or a DLC at index `0`. If a patch name you're deploying
is listed in your [Skip List](settings.md#skip-list), DDMM starts numbering that group's files at `.patch_1`
instead of `.patch_0`, leaving slot `0` alone. See [Settings reference](settings.md#skip-list) for when you'd add
an entry.

## Purge

Clicking **Purge** (tip: "Uninstall all mods from the game.") asks for confirmation ("Are you sure you want to
uninstall all mods from the game?"), then deletes **every** file in `<Game Path>/data/` that matches the
`<16 hex chars>.patch_N[.gpu_resources|.stream]` naming pattern — not just files DDMM itself deployed. This is
what "clean" means for deploy, and it's also available on its own if you just want your install back to a vanilla
state without deploying a new selection.

Purge only removes files matching that pattern; the rest of your `data` folder (and your game install as a whole)
is left alone.
