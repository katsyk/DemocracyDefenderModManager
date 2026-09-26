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
right away. Nexus Mods checks still need the optional sign-in or API key.

## 1. Pick where to import from

The Import window lists what it found on this PC:

- **Mods installed with another mod manager:** the folder where another Helldivers 2 mod manager keeps the
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
| **Another version is in DDMM** | Same Nexus mod and file, different version | No |
| **Same as "…"** | An identical copy of another item in the list, e.g. `Mod (1).zip` next to `Mod.zip` | Can't be ticked |
| **Older download of "…"** | You downloaded the same Nexus file more than once; only the newest is ticked | No |
| **No Helldivers 2 mod files inside** | An archive with no Helldivers 2 patch files or `manifest.json` (a Downloads folder holds other things too) | No |
| **Can't read: …** | A damaged archive, one with unsafe paths, or a broken `manifest.json` | Can't be ticked |

Several files from the same Nexus mod page (a main file and optional variants) are separate mods, and each one
is ticked.

To change the selection, use **Select All**, **Select None** and **Only New**, or type in the filter box. Below
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
- **A mod that fails** (for example an archive damaged partway through) doesn't stop the others. The summary at
  the end lists each one with the reason. You never get a separate error popup for every mod.
- **While an import runs**, other changes to DDMM's data wait, and moving the data folder is refused. If you
  try to close DDMM with the Import window open, it asks first.

## Adding many files at once

Picking 10 or more files with **Add**, or dropping 10 or more at once, uses the same list. You can check what
you're adding (duplicates, mods you already have) before anything is installed, and you get one summary. For
fewer files, Add installs them straight away as before.

## What's carried over

Different mod managers record different things. DDMM takes whatever the source has:

- **The mod itself:** its files, and its `manifest.json` if it has one. A mod without one gets a manifest
  built from what the other manager knew (its name and options), or else DDMM's usual
  [automatic one](adding-mods.md#archives-without-a-manifestjson).
- **Where it came from:** the Nexus Mods id, file and version, used for update checks. It comes from the other
  manager's records or from the Nexus download's file name. Nothing is looked up online during the import.
- **How it was set up:** on/off state, load order and chosen options, when the other manager keeps them in a
  form DDMM can read.
