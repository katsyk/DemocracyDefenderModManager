---
title: Adding mods
---

# Adding mods

DDMM treats an archive, a folder, and a direct link as equally first-class ways to add a mod. All of them end up
as an entry in your mod Library. Before you've added anything, the mod list itself says so: "No mods yet — Works
with AyakaMods, Nexus Mods, ModWorkshop, GameBanana, GitHub and more."

## Add (archive files)

Click **Add** (tip: "Add a mod to the library.") to open a file picker filtered to `.zip`, `.7z`, and `.rar`. You
can select one file or several at once — DDMM installs every archive you pick. With 10 or more files (picked or
dragged in), DDMM first shows the [import checklist](importing-mods.md), so duplicates and mods you already have
are sorted out before anything is installed.

Moving a whole collection over (another mod manager's mods, or a folder with hundreds of downloaded archives)?
Use [**Import**](importing-mods.md) instead.

## Add Folder

Click **Add Folder** (tip: "Add a mod from a plain folder.") to pick a folder that already contains a mod's files
unpacked — no archive step needed. You can pick multiple folders at once. DDMM copies the folder's contents
(symlinks are not followed) into its own managed `mods/` directory; your original folder is left untouched.

Pick **one mod's folder** at a time (or several single-mod folders at once), not a folder that holds your whole
collection. DDMM never copies a folder into itself, so it checks the folder you pick against its own `mods/`
folder (after following shortcuts/symlinks, `..`, and on Windows and macOS ignoring upper/lower case):

- **DDMM's own `mods/` folder, or a folder that contains it** (for example DDMM's data folder in a portable
  install): refused with an explanation, and nothing is copied. Mods in `mods/` that have a `manifest.json` are
  already in your list.
- **A mod folder sitting directly in `mods/`** that isn't in your list yet (it has no `manifest.json`, for
  example because you unpacked it there yourself): it's added **where it is**. Nothing is copied or moved; DDMM
  just writes a `manifest.json` into it.
- **A folder deeper inside one of your mods** (such as a mod's `Options/Red` folder): refused. Pick the mod's own
  folder instead.

If a copy fails partway (a file can't be read, the disk is full), DDMM removes the half-copied folder again.

## Add URL

Click **Add URL** (tip: "Add a mod from a direct download link.") and paste a mod page or download link — its
popup describes itself as: "Paste a mod page or download link from AyakaMods, Nexus Mods, ModWorkshop,
GameBanana, GitHub, or any direct download link."

A **direct** link (serving the archive file itself over `https://`) downloads and installs immediately, with a
2 GiB size cap, and a check that what comes back is actually a zip/7z/rar archive (by file signature, not just the
URL's extension). A mod **page** on a site that requires being logged in (AyakaMods, Nexus Mods) skips the direct
download attempt entirely and goes straight to a **browser handoff**; a page on another site that turns out not to
serve an archive directly gets offered the same handoff instead of just failing. See
[Mod sites](mod-sites.md) for exactly how that's decided and how the handoff works.

If a direct download attempt fails outright and no handoff is offered/accepted, you see:

> Adding mod from URL failed! — Use a direct download link, or download the file and add it manually.

## Drag & drop

Drag one or more archive files or folders onto the DDMM window (a "Drop archives or folders to add" overlay
appears while dragging) and drop them — DDMM installs each one, the same as picking them through Add / Add Folder.

## What happens after

Every install goes through the same steps:

1. A folder for the mod is created under `mods/`, named after the archive's filename (without its extension) or
   the source folder's name.
2. If the archive/folder contains a `manifest.json`, that becomes the mod's manifest (see
   [Manifest reference](../authors/manifest.md) for the formats DDMM understands).
3. If there's no `manifest.json`, DDMM generates a minimal one automatically: a random ID, the archive/folder name
   as the mod's name, an empty description — and, for this auto-generated manifest only, DDMM also scans the mod's
   files to fill in `Options` if needed; see [Archives without a manifest.json](#archives-without-a-manifestjson)
   below.
4. The mod is added to your Library, from where you can drag or insert it into a profile. If DDMM couldn't find
   any Helldivers 2 patch files anywhere in it, you'll see a non-fatal warning: "no Helldivers 2 patch files found
   in this archive" — the mod is still added, it just won't do anything when deployed.

If a mod with the same ID (`Guid`) is already installed, adding it again fails with a "mod with GUID ... already
exists" error rather than silently overwriting it.

## Archives without a manifest.json

For a mod installed with no `manifest.json` of its own (an auto-generated manifest — an author-supplied manifest
is never touched this way), DDMM scans the installed files to work out where its Helldivers 2 patch files
(`<16 hex chars>.patch_N`, plus `.gpu_resources`/`.stream` counterparts) actually live:

- **Patch files directly at the mod's root** — the common case — deploy as-is, no options.
- **No patch files at the root**, but one or more directories (up to 4 levels deep) directly contain some — each
  such directory becomes an entry in a Legacy-style `Options` list (the same single-choice dropdown described in
  [Mod options & variants](options-variants.md)), naturally sorted (so "Option 2" sorts before "Option 10") and
  defaulting to the first one. This covers both a **single wrapper folder** (e.g. everything nested one level down
  under a folder named after the mod) and **several variant folders** (e.g. `Red/`, `Blue/`) the same way — pick
  the one you want from the dropdown before deploying.
- **No patch files found anywhere** in the first 4 levels — the mod installs anyway, with the warning shown above,
  rather than silently doing nothing.

A directory that qualifies as an option isn't searched any further inside itself — whatever's in it is that
option's whole file tree, not a place to look for more nested variants.

## Batch adds and partial failures

When you add several archives, folders, or a mix of paths at once, each one is installed independently: one bad
archive doesn't stop the rest from installing. You'll see a result popup listing which succeeded and which
failed, with the error message for each failure.
