---
title: Data folder
---

# Data folder

DDMM keeps your installed mods (`mods/`), `settings.json`, `profiles.json`, its logs and a few small caches in one
**data folder**. Where that is by default depends on how you run DDMM; see
[Data location](../getting-started/download.md#data-location). You can move it anywhere you like, for example to
a bigger drive, since a Helldivers 2 mod collection can easily reach several GB.

## Seeing where it is

**Settings → Data Folder** shows the folder DDMM is using right now. **Open Folder** opens it in your file
manager. If you chose the folder yourself, the default location is shown underneath, with **Reset to Default**.

## Moving it

1. Open **Settings** and click **Change...** next to **Data Folder**.
2. Pick the folder you want DDMM to use. An empty folder, or a new one you create in the picker, is simplest.
3. DDMM checks the folder (see [what it refuses](#what-ddmm-refuses) below) and shows what it's about to do: the
   folder it moves from and to, how much data that is, and how much space is free there.
4. Click **Move**. A progress bar shows the copy. Don't close DDMM while it runs (DDMM ignores the close button
   until it's done).
5. DDMM restarts by itself, already using the new folder.

What happens during the move, in order:

1. Mod operations pause: installing, deleting, deploying, update checks, browser installs and auto-import all
   wait until DDMM restarts. A browser install that arrives meanwhile is told to try again later.
2. DDMM copies its own files into a temporary folder *inside* the destination.
3. It checks that every file arrived complete (same number of files, same sizes).
4. It moves the copy into place, then records the new location (see [How DDMM remembers the folder](#how-ddmm-remembers-the-folder)).
5. Only after that does it delete the old copy.
6. It restarts. Everything that refers to the data folder (mod images, the browser extension's connection, logs)
   follows the new folder from then on.

If anything fails before step 5 (the drive fills up or is unplugged, a file can't be read, and so on), DDMM
removes its partial copy, leaves your data exactly where it was, and tells you what went wrong. Nothing is ever
deleted from the old folder until the new one is complete and in use.

Moving between drives (for example from `C:` to `D:`, or to a USB drive) works the same way.

Only DDMM's own files are moved: `mods/`, `settings.json`, `profiles.json`, `logs/`, the update-check cache, and
(on Linux without a keychain) the owner-only files holding an optional Nexus Mods sign-in or API key. Anything
else in the old folder stays where it is. That matters for a portable copy, where the data folder is also the
folder with `ddmm.exe` in it, and on Linux, where the default folder also holds the app window's own cache.

If an old file can't be deleted straight away (on Windows, a log file that's still open), DDMM deletes it the
next time it starts.

### Picking a folder that isn't empty

- **A folder that already holds DDMM data** (it has DDMM's `settings.json` or `profiles.json`, for example a copy
  on a USB drive, or a folder you used before): DDMM offers **Use the Existing Data**. That switches DDMM to that
  folder and restarts. Nothing is copied or deleted, and your current data stays where it is, so you can switch
  back later. DDMM never copies over existing DDMM data.
- **Any other folder with files in it** (your Documents folder, a drive's root folder, ...): DDMM doesn't mix its
  files with yours. It creates a `DDMM Data` folder inside the one you picked and moves the data there. The
  confirmation says so before anything happens.

### What DDMM refuses

DDMM says why and changes nothing when the folder you pick:

- **is the current data folder, is inside it, or contains it.** Copying a folder into itself never finishes.
- **is inside, or contains, your Helldivers 2 install.** Keep DDMM's data outside the game folder.
- **can't be written to** (a read-only drive, or a folder you don't have permission for).
- **is on a drive without enough free space** for the data plus 64 MB of headroom.
- **contains an unfinished move** from an earlier attempt that was interrupted (for example by a power cut).
  Delete that folder after checking there's nothing in it you need, then try again.
- **contains a symbolic link or junction** inside DDMM's data. DDMM won't follow or drop links; replace the link
  with the real folder first, or move the data by hand.

## Resetting to the default location

With a folder you chose, **Settings → Data Folder** also shows **Reset to Default**. It moves the data back to the
default location in exactly the same way, then forgets the chosen folder. If the default location already holds
DDMM data (say, from before you moved it), you're offered to use that instead, as above.

## How DDMM remembers the folder

The chosen folder is stored in a small file named `ddmm-data-location.json`, outside the data folder itself so
it survives the move:

- **Installed** (Windows installer, `.deb`, `.rpm`, or an AppImage in a read-only location): in your per-user
  config folder, `%APPDATA%\io.github.katsyk.ddmm\` on Windows and `~/.config/io.github.katsyk.ddmm/` on Linux.
- **Portable**: next to `ddmm.exe` (or the `.AppImage`), so the portable folder describes itself.

When DDMM starts it picks its data folder in this order, first match wins:

1. `ddmm-data-location.json` next to the executable;
2. the executable's own folder, if it's a [portable copy](../getting-started/download.md#data-location) and
   writable;
3. `ddmm-data-location.json` in the per-user config folder;
4. the per-user app data folder.

So a location you chose always wins over the default, and a portable copy never picks up an installed copy's
choice. The browser extension's helper uses the same rules, so it finds DDMM after a move without any repair.

!!! note "Portable copies in a read-only folder"
    A portable copy only uses its own folder for data while that folder is writable (otherwise it behaves like an
    installed copy). Moving the data works either way: when the program's folder is read-only, the location is
    stored in the per-user config folder instead. Once `ddmm-data-location.json` sits next to the executable, the
    program's folder no longer needs to be writable at all.

### Moving a whole portable folder

If you move or rename the portable folder (the one with `ddmm.exe` and `ddmm-data-location.json` in it):

- **Data somewhere else** (for example `D:\DDMM Data`): nothing changes. The location file still points there.
- **Data in a folder inside the portable folder** (for example `...\DDMM\Data`): the location is stored
  *relative* to the portable folder, so it keeps working after the move.
- **Data in the portable folder itself** (you never chose another folder): there's no location file, and the
  data moves with the folder as always.

## If the folder is missing at startup

If DDMM starts and the chosen folder isn't there, for example because the USB drive it's on is unplugged or a
drive letter changed, DDMM does **not** create a new empty folder (which would look like all your mods were gone).
Instead it shows **Data Folder Not Found**, with the missing path and these options:

- **Retry**: connect the drive, then click this. DDMM restarts normally if the folder is back.
- **Locate Folder...**: pick where the data is now (for example the same folder under a new drive letter). DDMM
  only accepts a folder that holds DDMM data, or one with a `DDMM Data` folder in it.
- **Use the Default Location**: forget the chosen folder and restart with the default location, which may be
  empty. The missing folder isn't touched, so you can switch back to it later with **Change...** and
  **Use the Existing Data**.
- **Quit**.

The same screen appears if `ddmm-data-location.json` itself is damaged. Until you choose, DDMM doesn't touch
any data folder: no mod list, no browser connection, no update checks. That session's log goes to a
`io.github.katsyk.ddmm-logs` folder in your system's temp folder.

## Moving it by hand

You can also move the data yourself: close DDMM, move the folder, start DDMM, and use **Locate Folder...** on the
recovery screen (for a folder you chose before), or **Change...** → **Use the Existing Data** (otherwise). Don't
edit `ddmm-data-location.json` by hand unless you have to; if you do, it looks like
`{"Version": 1, "Path": "D:\\DDMM Data"}`.
