---
title: Importing mods
---

# Importing mods

**Import** on the Mods page brings over many mods at once. It works from another mod manager's folder, or from
a folder of mod archives you downloaded earlier. You see a list and tick what you want. DDMM copies it all in,
with one progress bar and one summary at the end.

DDMM never downloads anything during an import. It uses only files already on your PC, and it only reads
them: nothing in the other mod manager's folder or in your downloads is changed, moved or deleted.

## "I have hundreds of Nexus mods. How do I move them?"

You don't have to add them one by one.

- **Another mod manager already has your mods:** click **Import**. DDMM looks in the usual places and lists
  what it finds, for example "Mods installed with another mod manager — Found 312 mods in
  C:\Users\you\AppData\Local\...". Click **Scan**, check the list, then click **Import 312 Mods**.
- **Your mods are archives in a folder** (Downloads, or wherever you kept the `.zip` / `.7z` / `.rar` files):
  click **Import**, then **Choose a Folder...** and pick that folder. Subfolders are searched too.

When the mod list is empty, it offers the same **Import Mods** button and names any folders it already found.

A Nexus Mods download keeps the mod's Nexus id and version in its file name, and other mod managers record
them too. Either way, DDMM keeps that information, so [update checks](updating-mods.md) work for imported mods
right away. Nexus Mods checks still need the optional API key (or, once available, sign-in).

## 1. Pick where to import from

The Import window lists what it found on this PC:

- **Mods installed with another mod manager:** the folder where another mod manager for Helldivers 2 keeps the
  mods it installed.
- **Archives downloaded by another mod manager:** the folder where one keeps the original downloads.
- **Your downloads folder:** the Downloads folder from DDMM's Settings, or your system's.

Each line says how many mods are there. **Choose a Folder...** takes any folder. That works best for:

- a mod manager that keeps its data somewhere unusual, or in a folder you chose;
- a Linux setup where the other manager runs under Wine/Proton;
- your own folder of archives.

DDMM refuses a folder that is, contains, or is inside its own [data folder](data-folder.md). Importing from
there would copy DDMM's mods into themselves.

## 2. Check the list

DDMM reads every archive and mod folder without unpacking anything, then shows each one with its name, size,
Nexus Mods id and version (when known), and a status:

| Status | Meaning | Ticked? |
|---|---|---|
| **New** | Not in DDMM yet | Yes |
| **Already in DDMM as "…"** | The same mod is installed (same ID, same archive, same file name, or same Nexus file) | No |
| **Already in DDMM: will add Nexus info** | You already added this mod yourself (for example with Add), but DDMM doesn't know its Nexus page yet; the import knows it | Yes |
| **Has the ID of "…" in DDMM: can replace it** | An archive whose `manifest.json` has the ID of an installed mod, and that isn't the file that mod came from. Ticking it **replaces** that mod; see [Replacing mods you already have](#replacing-mods-you-already-have) | No, never (not even by Select All) |
| **Another version is in DDMM** | Same Nexus mod and file, different version (but no shared ID, so it can't replace: use [Update](updating-mods.md) on the mod) | No |
| **Same as "…"** | An identical copy of another item in the list (the very same files), e.g. `Mod (1).zip` next to `Mod.zip` | Can't be ticked |
| **Older version of "…"** | Another version of a mod that's also in the list: the same mod ID with different files, or the same Nexus file downloaded more than once. Only the newest is ticked: the one Nexus uploaded last, else the higher version number, else the newer file | No |
| **No Helldivers 2 mod files inside** | An archive with no Helldivers 2 patch files or `manifest.json` (a Downloads folder holds other things too) | No |
| **Can't read: …** | A damaged archive, one with unsafe paths, or a broken `manifest.json` | Can't be ticked |

Several files from the same Nexus mod page (a main file and optional variants) are separate mods, and each one
is ticked.

If the folder you pick is a single mod itself (it has its own `manifest.json`, with its options in subfolders),
the list shows just that one mod.

To change the selection, use **Select All**, **Select None** and **Only New**, or type in the filter box. **Select
All** ticks everything except files that would replace a mod you have (those keep whatever you set). Below
the list you see how much space the selection needs once unpacked, and how much is free on the drive with
DDMM's data folder. If it won't fit, the import stops before it starts and tells you why.

**Also add them to the profile "…"** puts the imported mods into your current [profile](profiles.md), ready to
deploy. When the other mod manager recorded it, they keep the same load order, the same on/off state and the
same chosen [options](options-variants.md). Leave it unticked to put them only in your Library.

## 3. Import

DDMM imports the mods one at a time, with a progress bar showing how many are done and how much has been
copied.

- **Cancel** stops after the mod being copied at that moment, which is removed again. Everything finished before
  that stays. To get the rest later, run Import again: what's already there is recognized and left unticked.
  A **replace** can't be undone: if Cancel is pressed while a mod is being replaced, that replace finishes and
  the import stops after it. Each replace is all-or-nothing on its own, and the summary lists every mod that
  was replaced, together with a note that they stay replaced.
- **A mod that fails** (for example an archive damaged partway through) doesn't stop the others. The summary at
  the end lists each one with the reason. You never get a separate error popup for every mod.
- **While an import runs**, other changes to DDMM's data wait, and moving the data folder is refused. If you
  try to close DDMM with the Import window open, it asks first.

## Replacing mods you already have

Every mod has an ID (the `Guid` in its `manifest.json`), and two installed mods can't share one. An archive in the
list with the ID of a mod you already have (and that isn't the very file that mod came from) can **replace** it.
It's the bulk version of the question DDMM asks when you
[add such a file on its own](adding-mods.md#replacing-an-installed-mod):

- The row names both mods and their versions when known: *Can replace "Cool Mod" (version 1.0) with this file
  ("Cool Mod" (version 2.0))*.
- **Downgrade** is shown in yellow when the file is older than the installed mod (a lower version number, or an
  earlier Nexus upload). The same goes for an *Older version of "…"* row that has the installed mod's ID: it can
  replace it too, and is labelled the same way.
- **Different names**: the row warns that these may be two different mods that share an ID (an author copied
  another mod's `manifest.json`). Replacing removes the installed one and puts the new one in its place.

These rows are **never ticked for you**: not by default, not by **Only New**, and not by **Select All**. Each
replace removes an installed mod's files, and may put a different mod in its place, so you tick each one
yourself. Ticking a row means *replace*. Only one file can replace a given mod: ticking a second file for the
same mod unticks the first.

A ticked row goes through exactly the same swap as **Replace** on a single file: the file is unpacked and checked
completely first (if anything goes wrong, the installed mod stays as it was), and the mod keeps its ID and folder,
so its **place in every profile**, whether it's **on**, and its **option choices** that still fit (the rest are
reset to the defaults). The other mod manager's load order and on/off state aren't applied to a replaced mod.
Where the mod came from is recorded afresh for the new file, and what the last update check said about it is
cleared. If the installed mod was removed after the scan, the file is imported as a new mod instead.

The summary lists replaced mods separately. When one of them is on in your active profile, DDMM redeploys once at
the end (or asks to), [as after an update](updating-mods.md#after-updating-redeploy).

**Mod folders can't replace.** Replacing works on archives only. A mod folder (for example, one installed by
another mod manager) with the ID of a mod you have stays *Already in DDMM*, and the list says so. To use that
folder's version, remove the installed mod first and import again.

## Mods you already added by hand

Added a pile of archives with **Add** before you found Import? Run Import on the same folder anyway. Mods that
are already in DDMM aren't copied again. When DDMM has no Nexus Mods link for one of them and the import source
has one (from the Nexus download's file name, or the other mod manager's records), it's listed as **Already in
DDMM: will add Nexus info**, and it's ticked. Importing it only records the Nexus page, version and file, so
[update checks](updating-mods.md) cover it. Its files, options, place in the load order and on/off state stay
exactly as they are.

## Adding many files at once

Picking 10 or more files with **Add**, or dropping 10 or more at once, uses the same list. You can check what
you're adding (duplicates, mods you already have) before anything is installed, and you get one summary. For
fewer files, Add installs them straight away as before.

If you stop that scan (or it fails), DDMM says so and offers **Scan Again**, **Choose a Folder Instead** or
**Close**. Files dropped while the Import window (or any other DDMM window) is open are ignored, with a short
note: finish or close it first, then drop them again.

## What's carried over

Different mod managers record different things. DDMM takes whatever the source has:

- **The mod itself:** its files, and its `manifest.json` if it has one. A mod without one gets a manifest
  built from what the other manager knew (its name and options), or else DDMM's usual
  [automatic one](adding-mods.md#archives-without-a-manifestjson).
- **Where it came from:** the Nexus Mods id, file and version, used for update checks. It comes from the other
  manager's records or from the Nexus download's file name. Nothing is looked up online during the import.
- **How it was set up:** on/off state, load order and chosen options, when the other manager keeps them in a
  form DDMM can read.
