---
title: Troubleshooting
---

# Troubleshooting

## DDMM won't start (Linux)

Run DDMM from a terminal to see why. Most often a library is missing, for example
`error while loading shared libraries: libwebkit2gtk-4.1.so.0` from the `.tar.gz` when WebKitGTK 4.1 isn't
installed. An AppImage window can also stay blank with an `EGL_BAD_PARAMETER` error. [Installing on
Linux](../getting-started/linux.md) covers which download to use for your distribution, the exact package to
install for each one, AppImage/FUSE errors, and the blank-window workaround.

## Windows SmartScreen warns about an unknown publisher

Expected for these builds — they aren't code-signed. Click **"More info"**, then **"Run anyway"**. This applies to
both the installer and the app itself the first time you run it.

## DDMM opens straight to Settings

On startup DDMM checks your [Game Path](../using/settings.md#game-path); if it's invalid or empty (including on a
completely fresh install), it first tries to auto-detect your Helldivers 2 install via Steam and, if that
succeeds, fills in and saves the path for you automatically — you'll see a "Found Helldivers 2 at ..." message
and DDMM continues straight to Mods, no trip to Settings needed.

You only land on Settings when auto-detection also fails (Steam isn't installed, or the game isn't). Set the path
yourself, or click **Auto-detect** again after installing/moving the game — see
[First-time setup](../getting-started/setup.md).

## A mod's manifest can't be read

DDMM reads each mod's `manifest.json` when you add the mod and every time it starts. It accepts the usual
hand-editing quirks — a UTF-8 byte-order mark, UTF-16 files, `//` comments, trailing commas, `GUID`/`iconPath`
style key spellings, `"Version": "1"`, a missing `Description`, `Manifest.json` instead of `manifest.json`, and
Windows `\` paths. When a manifest still can't be read, the error names the mod and, for a JSON syntax error,
the line and column, e.g.

```text
Couldn't install "Some Mod.zip".
Step: reading its manifest.json
Cause: manifest.json is not valid JSON: expected ',' or '}' at line 4 column 5
Hint: The mod's manifest.json is broken: let the mod's author know. ...
```

If that happens while **adding** a mod, the mod isn't installed; the popup and the [log](logs.md) show the reason.
Report it to the mod's author (and to us, see [Reporting bugs](bugs.md), if you think DDMM should accept it).

If an already-installed mod's manifest becomes unreadable (e.g. you edited it by hand), DDMM skips just that mod
when loading the list and logs `Skipping mod in "...": <reason>` — the rest of your mods still load. Fix or
re-download that mod: click **Open Folder** next to **Data Folder** in Settings, go into `mods/`, and fix or
delete its folder.

## A mod won't install

When adding a mod fails, the popup (and the [log](logs.md)) says which file, which step failed (downloading it,
opening the archive, reading its manifest.json, extracting it, copying its files, ...), why in plain words, and
what to do about it:

```text
Couldn't install "koyuki launcher.zip".
Step: opening the archive
Cause: the zip archive is incomplete or damaged: its table of contents (at the end of the file) is missing, which
usually means the download didn't finish (invalid Zip archive: Could not find EOCD)
Hint: Download the mod again and add the new file. ...
```

The text in parentheses is the archive tool's own message; include it if you [report a bug](bugs.md). Common
causes:

- **The download didn't finish**, or the site sent a web page (a login or "please wait" page) instead of the
  file. Download it again from the mod's page.
- **Password-protected archives**, **archives split into parts** (`.7z.001`, `.z01`, `.part2.rar` missing) and
  7z archives packed with **Deflate64** can't be installed directly. Extract them yourself (for example with
  7-Zip), then add the extracted folder with [Add Folder](../using/adding-mods.md).
- **Two mods with the same ID.** Some authors copy one mod's `manifest.json` into their next mod, so both claim
  the same GUID. DDMM names the mod already using it. Tell the author; to install it anyway, extract the
  archive, delete its `manifest.json`, and add the folder with Add Folder (DDMM then gives it an ID of its own).
- **An archive of archives** (one zip per variant inside the download) installs with a warning that names the
  inner archives. Extract the one you want and add that instead.

DDMM opens a zip, 7z or RAR by what's inside, not by its name, so a RAR uploaded as `.zip` installs fine.

## A mod I added doesn't show up

- A mod folder under `mods/` with **no** `manifest.json` in it at all is silently skipped when DDMM loads the mod
  list (rather than erroring) — this can happen if an install was interrupted. Delete the incomplete folder and
  re-add the mod.
- Check the [log file](logs.md) — a failed add shows a popup with the specific error, and the same detail is in
  the log.

## My mods folder filled the disk with nested copies of itself

Versions up to and including 2.0.0-rc.6 had a bug: using **Add Folder** (or dragging a folder in) on DDMM's own
`mods/` folder, or on a folder that contains it, made DDMM copy that folder into itself. The copy kept reading
what it had just written, so it nested a full copy of every mod inside the previous copy, over and over, until the
path got too long or the disk filled up. A 1 GB mod folder can end up taking hundreds of GB. This is most likely
when DDMM runs [portable](../getting-started/download.md#data-location) next to an existing `mods/` folder and you
point **Add Folder** at that folder. Newer versions refuse to do this (see
[Add Folder](../using/adding-mods.md#add-folder)).

The duplicates all live under **one** extra folder, directly inside `mods/`, that is named after the folder you
picked:

- You picked `mods/` itself: the extra folder is `mods/mods/`, and inside it is `mods/mods/mods/`, and so on. Each
  level holds another copy of all your mods.
- You picked a folder that contains `mods/`, say `MyMods/`: the extra folder is `mods/MyMods/`, and the nesting
  goes `mods/MyMods/mods/MyMods/...`.

Your real mods are the other folders directly inside `mods/`. They were only ever read, never changed. To clean
up:

1. Close DDMM and update it to a version with the fix.
2. Find the data folder: **Open Folder** next to **Data Folder** in Settings, then go into `mods/`.
3. Look inside the extra folder (`mods/mods/` or `mods/<name you picked>/`) and check that it only holds copies of
   mods you still have directly in `mods/`. If you really had a mod of your own named `mods`, only its own files
   at the top of that folder are real. The copies start one level down.
4. Delete that one extra folder. Don't delete anything else. It can be too deep for File Explorer ("path too
   long"), and too big for the Recycle Bin, so use a terminal:
    - **Windows** (Command Prompt, not PowerShell). The `\\?\` prefix lets it remove very long paths:
      `rmdir /s /q "\\?\C:\full\path\to\mods\mods"`. If that still fails, mirror an empty folder over it first,
      then remove both: `mkdir "%TEMP%\empty"`, then
      `robocopy "%TEMP%\empty" "C:\full\path\to\mods\mods" /MIR`, then
      `rmdir /s /q "\\?\C:\full\path\to\mods\mods"` and `rmdir "%TEMP%\empty"`.
    - **Linux:** `rm -rf -- "/full/path/to/mods/mods"`.
5. Start DDMM. Mods directly in `mods/` that have a `manifest.json` show up as before. For any that don't, use
   **Add Folder** on that mod's own folder: DDMM adds it where it is, without copying it.

## Deploy or Purge fails immediately

Both refuse to run against an invalid [Game Path](../using/settings.md#game-path) — double-check Settings first.
If the path is valid but deploy still fails partway through, check the log for which file operation failed (for
example, a patch file the mod claims to include that doesn't actually exist in its folder).

## A specific mod won't deploy, or deploying it errors out

`V1` and `V2` manifests deploy the same way. If an enabled option (or the selected sub-option) doesn't actually
contribute any files, check the [log file](logs.md) for a warning naming the mod and an out-of-range option/
sub-option index, or an `Include` folder that doesn't exist in the mod's own directory — deploy skips that
option rather than failing the whole deploy, but it also means nothing gets installed for it.

## Browser handoff fails immediately with a Downloads-folder error

"the downloads folder isn't set or doesn't exist -- check it in Settings" means your
[Downloads Folder](../using/settings.md#downloads-folder) setting is empty or points at a folder that no longer
exists — fix it in Settings, then retry.

## "a handoff is already in progress"

Only one [browser handoff](../using/mod-sites.md#how-the-browser-handoff-works) can run at a time. Cancel or wait
for the current one (Waiting/Installing popup) to finish before starting another.

## Closing DDMM says "Saving profiles..." but the window never closes

Fixed in rc.4 — a missing window permission meant the app could save your profiles and then silently fail to
actually close itself afterward, leaving the window stuck open with no visible error. If you're still on rc.3 or
earlier, update DDMM. On a version with the fix, closing that still doesn't finish within a few seconds instead
shows a "Couldn't save profiles" or "Deploy in progress" prompt asking whether to close anyway — check the
[log file](logs.md) for the underlying error if that keeps happening.

## Linux (CachyOS, Arch, Steam Deck, and others)

For installing DDMM itself (which file, dependencies, AppImage, blank window), see
[Installing on Linux](../getting-started/linux.md). This section covers problems once DDMM is running.

### "Game path is invalid!" even though the path is right

Fixed in 2.0.0-rc.6. Earlier versions checked the path in a way that could never see inside
hidden folders on Linux — and the default Steam library is under `~/.local/share/Steam` (or `~/.steam/steam`), so
every default install was rejected. Update DDMM. On a fixed version, the message under the field says exactly what's wrong
(see [First-time setup](../getting-started/setup.md#setting-the-game-path-by-hand)).

Where the game usually is:

| Steam install | Game Path |
| --- | --- |
| Native package (`steam` from pacman/CachyOS, Debian, ...) | `~/.local/share/Steam/steamapps/common/Helldivers 2` |
| Flatpak (`com.valvesoftware.Steam`) | `~/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Helldivers 2` |
| Snap | `~/snap/steam/common/.local/share/Steam/steamapps/common/Helldivers 2` |
| Extra library on another drive | `<library>/steamapps/common/Helldivers 2` (see Steam → Settings → Storage) |

**Auto-detect** checks all of the above plus every library listed in Steam's `libraryfolders.vdf`. Paths with
spaces, non-English characters, and symlinked libraries all work. In the folder picker, press `Ctrl+H` to show
hidden folders like `.local`, or just paste the path into the field (`~` isn't expanded — use the full
`/home/<you>/...` form).

DDMM only needs to read and write the game's `data/` folder; it doesn't care whether Steam is native or Flatpak,
or which Proton version runs the game.

### Game on an NTFS or exFAT drive (dual boot)

- If the drive is mounted **read-only** — Linux does this to an NTFS volume Windows left "dirty" (Fast Startup or
  hibernation) — deploying fails with `Read-only file system`. Disable Fast Startup in Windows, shut Windows down
  fully, and remount.
- The drive must be mounted with your user as owner (the default for drives mounted from your file manager). If
  deploying fails with `Permission denied`, check the mount options (`uid=`/`gid=` for `ntfs3`/`exfat`).
- Letter case: NTFS keeps the names Windows created (`data`, `bin`, `helldivers2.exe`), which is what DDMM expects.

### Adding a mod fails

Every failure now shows the real reason in the popup (older versions only said `errors disabled in production.`)
and writes it to the log. Mods downloaded from a URL are staged in a `.downloads` folder inside the data folder
(not `/tmp`, which is a small RAM disk on Arch/CachyOS), and DDMM copies instead of moving whenever a file has to
cross from one drive to another.

### Where the log is on Linux

With the `.tar.gz`, `.deb`, `.rpm` or AppImage, the [data folder](../getting-started/download.md#data-location) is
normally `~/.local/share/io.github.katsyk.ddmm/`, so the log is
`~/.local/share/io.github.katsyk.ddmm/logs/Democracy Defender Mod Manager.log`. Exception: if you extracted the
`.tar.gz` into a folder you can write to that already has DDMM data (`mods/`, `settings.json`) or a
`portable.txt` next to the executable, DDMM runs portable and the log is in `logs/` next to the executable. The
first lines of the log say which one it picked; **Open Folder** next to **Data Folder** in Settings always opens
the right one. If you [moved the data folder](../using/data-folder.md), the log is in `logs/` inside the folder you
chose.

### "Data Folder Not Found" when DDMM starts

DDMM's data is in a folder you chose, and that folder isn't there: typically a USB or external drive that isn't
connected, or a changed drive letter. DDMM hasn't created or deleted anything. Connect the drive and click
**Retry**, or use **Locate Folder...** to point DDMM at where the data is now. See
[If the folder is missing at startup](../using/data-folder.md#if-the-folder-is-missing-at-startup).

Running DDMM from a terminal (`./ddmm`) also prints the log live.

## Still stuck?

See [Reporting bugs](bugs.md).
