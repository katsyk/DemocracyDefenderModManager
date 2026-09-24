---
title: Adding mods
---

# Adding mods

DDMM treats an archive, a folder, and a direct link as equally first-class ways to add a mod. All of them end up
as an entry in your mod Library.

## Add (archive files)

Click **Add** (tip: "Add a mod to the library.") to open a file picker filtered to `.zip`, `.7z`, and `.rar`. You
can select one file or several at once — DDMM installs every archive you pick.

## Add Folder

Click **Add Folder** (tip: "Add a mod from a plain folder.") to pick a folder that already contains a mod's files
unpacked — no archive step needed. You can pick multiple folders at once. DDMM copies the folder's contents
(symlinks are not followed) into its own managed `mods/` directory; your original folder is left untouched.

## Add URL

Click **Add URL** (tip: "Add a mod from a direct download link.") and paste a link. This only works for links that
serve the archive file directly over `https://` — DDMM downloads it itself, with a 2 GiB size cap, and checks
that what comes back is actually a zip/7z/rar archive (by file signature, not just the URL's extension) before
installing it. If a site instead serves an HTML page (a login wall, a page with a JavaScript-driven download
button), the download is rejected with:

> Adding mod from URL failed! — Use a direct download link, or download the file and add it manually.

Many mod sites don't expose a direct link at all; see [Mod sites](mod-sites.md) for how DDMM handles those instead.

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
   as the mod's name, an empty description, and no options — so the mod still shows up and can be deployed.
4. The mod is added to your Library, from where you can drag or insert it into a profile.

If a mod with the same ID (`Guid`) is already installed, adding it again fails with a "mod with GUID ... already
exists" error rather than silently overwriting it.

## Archives without a manifest.json

DDMM locates Helldivers 2 patch files (`<16 hex chars>.patch_N`, plus their `.gpu_resources`/`.stream`
counterparts) directly inside a mod folder even when there's no `manifest.json` at all — that's how an
unmanifested mod still deploys correctly.

!!! info "Landing feature"
    Support for treating multiple top-level variant folders (e.g. `Red/`, `Blue/`) inside such an archive as
    selectable options, and for warning (rather than silently installing nothing) when no patch files are found,
    is landing alongside this documentation — check the
    [release notes](https://github.com/katsyk/DemocracyDefenderModManager/releases) for whether it's shipped in
    your version.

## Batch adds and partial failures

When you add several archives, folders, or a mix of paths at once, each one is installed independently: one bad
archive doesn't stop the rest from installing. You'll see a result popup listing which succeeded and which
failed, with the error message for each failure.
