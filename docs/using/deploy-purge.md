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
2. Walks your active profile's mod list, skipping any mod whose toggle is switched off.
3. For each enabled mod, collects its patch files — from the mod's root, from the selected legacy option's
   subfolder, or from each toggled `V1`/`V2` option's (and selected sub-option's) `Include` folders, depending on
   its [manifest](../authors/packaging.md#folder-layout-by-manifest-type) — grouped by their 16-character patch
   name. `V1` and `V2` collect identically; `V2`'s `Categories`/`CategoryRef` only affect how options are grouped
   in the options editor, not what gets deployed. If a mod can't be collected (for example, an option folder its
   manifest names is missing), deploy stops here with an error, before your `data` folder is touched, so the
   mods you deployed last time stay in place.
4. **Purges** (see below), so every deploy starts from a clean `data` folder rather than layering on top of a
   previous one.
5. Copies each group's patch/`.gpu_resources`/`.stream` files into `<Game Path>/data/`, numbering them
   `.patch_0`, `.patch_1`, and so on in your profile's mod order. If a triplet is missing its `.gpu_resources` or
   `.stream` file, DDMM writes an empty placeholder for it instead of skipping it, so the numbering for later
   mods touching the same patch name stays consistent. Higher numbers override lower ones in game, so the mod
   lowest in your list wins a conflict (see [Load order and conflicts](profiles.md#load-order-and-conflicts)).

Deploying an empty profile (no mods, or none enabled — same list of `Configs`) shows an error instead of purging
your game folder for nothing: "Can not deploy empty profile!"

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
state without deploying a new selection. The one exception is slot `0` of a patch name in your
[Skip List](settings.md#skip-list): that `.patch_0` (and its `.gpu_resources`/`.stream`) belongs to the game, so
purge leaves it in place — unless DDMM put it there itself (deployed before you added the name to the Skip List),
in which case it is purged like any other mod file.

To tell the two apart, each deploy writes a small record of the files it wrote, `.ddmm-deployed.json`, into the
same `data` folder; purge removes it again. Without that record (for example after a deploy by an older version)
a skip-listed slot `0` file is always kept.

!!! warning "Upgrading from rc.14 or earlier"
    If you used rc.14 or earlier with Skip List entries, verify game files in Steam once, because older versions could delete the game's own files for those names.

Purge only removes files matching that pattern (and its own `.ddmm-deployed.json` record); the rest of your `data`
folder (and your game install as a whole) is left alone.
